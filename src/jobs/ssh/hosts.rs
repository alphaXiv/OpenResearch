//! Hosts the user saves in OpenResearch, and the `ssh -F` file that puts them
//! ahead of the user's own SSH configuration without editing it.

use std::path::{Path, PathBuf};

use crate::error::{anyhow, Result};

/// Where a saved host appears: Connect to remote, or the compute host list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum List {
    Remotes,
    Compute,
}

impl List {
    pub(crate) fn path(self) -> PathBuf {
        directory().join(match self {
            Self::Remotes => "remotes",
            Self::Compute => "compute",
        })
    }

    fn other(self) -> Self {
        match self {
            Self::Remotes => Self::Compute,
            Self::Compute => Self::Remotes,
        }
    }
}

fn directory() -> PathBuf {
    crate::config::config_dir().join("ssh")
}

/// The generated `ssh -F` file: saved hosts first, then everything plain `ssh`
/// reads. `system` adds the system configuration, which a UID-mapped user
/// namespace shows as nobody-owned and OpenSSH then refuses.
pub(crate) fn config_file(system: bool) -> Result<PathBuf> {
    let path = directory().join(if system {
        "config"
    } else {
        "config-without-system"
    });
    let contents = config_contents(system);
    if std::fs::read(&path).ok().as_deref() != Some(contents.as_bytes()) {
        std::fs::create_dir_all(directory())?;
        crate::local::git::atomic_write_with_mode(&path, contents.as_bytes(), Some(0o600))
            .map_err(|error| anyhow!("Could not write {}: {error}", path.display()))?;
    }
    Ok(path)
}

/// The variant without the system configuration, when `config` is the one with it.
#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn without_system(config: &Path) -> Option<Result<PathBuf>> {
    (config == directory().join("config")).then(|| config_file(false))
}

fn config_contents(system: bool) -> String {
    let mut own = Vec::new();
    if let Some(home) = crate::config::home_dir() {
        own.push(home.join(".ssh").join("config"));
    }
    if system {
        own.push(system_config());
    }
    let mut contents = format!(
        "# Managed by OpenResearch: hosts saved in OpenResearch, then your own SSH configuration.\nInclude {} {}\n",
        quote_path(&List::Remotes.path()),
        quote_path(&List::Compute.path()),
    );
    if !own.is_empty() {
        let own = own.iter().map(|path| quote_path(path)).collect::<Vec<_>>();
        contents.push_str(&format!("Host *\n  Include {}\n", own.join(" ")));
    }
    contents
}

#[cfg(unix)]
fn system_config() -> PathBuf {
    PathBuf::from("/etc/ssh/ssh_config")
}

#[cfg(windows)]
fn system_config() -> PathBuf {
    crate::local::shell_env::var("PROGRAMDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
        .join("ssh")
        .join("ssh_config")
}

/// Win32-OpenSSH reads forward slashes in config paths; backslashes would be escapes.
fn quote_path(path: &Path) -> String {
    let path = path.to_string_lossy();
    #[cfg(windows)]
    let path = path.replace('\\', "/");
    config_quote(&path)
}

pub(crate) fn config_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// OpenSSH's argument quoting, escapes and word-start comments.
pub(crate) fn config_args(value: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut arg = String::new();
    let mut quote = None;
    let mut started = false;
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' if quote.is_none() => {
                if started {
                    args.push(std::mem::take(&mut arg));
                    started = false;
                }
            }
            '#' if !started => break,
            '\\' if chars.peek().is_some_and(|&next| {
                matches!(next, '\\' | '\'' | '"') || (quote.is_none() && next == ' ')
            }) =>
            {
                arg.push(chars.next().unwrap());
                started = true;
            }
            '\'' | '"' if quote.is_none() => {
                quote = Some(c);
                started = true;
            }
            c if quote == Some(c) => quote = None,
            c => {
                arg.push(c);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return Vec::new();
    }
    if started {
        args.push(arg);
    }
    args
}

/// One `Host` block: the alias and its options in the order written, each
/// value as config text (quoted where needed), since ssh reads `-o` that way too.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Block {
    pub host: String,
    pub options: Vec<(String, String)>,
}

impl Block {
    fn render(&self) -> String {
        let mut text = format!("Host {}\n", self.host);
        for (key, value) in &self.options {
            text.push_str(&format!("  {key} {value}\n"));
        }
        text
    }

    fn same_as(&self, other: &Block) -> bool {
        let normalize = |options: &[(String, String)]| {
            options
                .iter()
                .map(|(key, value)| (key.to_ascii_lowercase(), value.clone()))
                .collect::<std::collections::BTreeSet<_>>()
        };
        normalize(&self.options) == normalize(&other.options)
    }
}

/// Turns what a user pastes (`ssh …`, `ssh://…`, `user@host[:port]` or an
/// alias) into the `Host` block that makes `ssh <alias>` connect the same way.
pub(crate) fn parse(input: &str) -> Result<Block> {
    let input = input.trim();
    if input.is_empty() {
        return Err(anyhow!("Enter a hostname or an ssh command."));
    }
    if input.contains(['\n', '\r']) {
        return Err(anyhow!("Enter a single ssh command."));
    }
    #[cfg(windows)]
    let input = &literal_backslashes(input);
    let words = shell_words::split(input)
        .map_err(|_| anyhow!("Could not read that ssh command: check its quotes."))?;
    let mut words = words.into_iter().peekable();
    let command = words.peek().is_some_and(|first| {
        first == "ssh" || first.ends_with("/ssh") || first.eq_ignore_ascii_case("ssh.exe")
    });
    if command {
        words.next();
    }

    let mut options: Vec<(String, String)> = Vec::new();
    let mut destination = None;
    // The first user and port win in ssh, so the destination's go where it was written.
    let mut destination_at = 0;
    // Like ssh, options may follow the destination; the next plain word starts the command.
    while let Some(word) = words.next() {
        if word == "--" {
            if destination.is_none() {
                destination = words.next();
                destination_at = options.len();
            }
            break;
        }
        let Some(flags) = word.strip_prefix('-').filter(|flags| !flags.is_empty()) else {
            if destination.is_some() {
                break;
            }
            destination = Some(word);
            destination_at = options.len();
            continue;
        };
        for (index, flag) in flags.char_indices() {
            if let Some((key, value)) = flag_option(flag) {
                options.push((key.into(), value.into()));
                continue;
            }
            if "tTNnqvfgsyMVG".contains(flag) {
                continue;
            }
            if !"pliJobBcmELRDFSOQwWeI".contains(flag) {
                return Err(anyhow!("Unknown ssh option -{flag}."));
            }
            let rest = &flags[index + flag.len_utf8()..];
            let value = if rest.is_empty() {
                words
                    .next()
                    .ok_or_else(|| anyhow!("ssh option -{flag} needs a value."))?
            } else {
                rest.to_owned()
            };
            if value.is_empty() {
                return Err(anyhow!("ssh option -{flag} needs a value."));
            }
            match flag {
                'p' => options.push(("Port".into(), port(&value)?.to_string())),
                'l' => options.push(("User".into(), config_value(&value))),
                'i' => {
                    // ssh read it from the shell's directory; orx runs elsewhere.
                    if !value.starts_with('~') && !Path::new(&value).is_absolute() {
                        return Err(anyhow!(
                            "Use the full path to {value} for -i, starting with / or ~."
                        ));
                    }
                    options.push(("IdentityFile".into(), config_value(&value)))
                }
                'J' => options.push(("ProxyJump".into(), config_value(&value))),
                'b' => options.push(("BindAddress".into(), config_value(&value))),
                'B' => options.push(("BindInterface".into(), config_value(&value))),
                'c' => options.push(("Ciphers".into(), config_value(&value))),
                'm' => options.push(("MACs".into(), config_value(&value))),
                'o' => {
                    if let Some(option) = config_option(&value)? {
                        options.push(option);
                    }
                }
                // Forwards and logging belong to one session, not to the host.
                'E' | 'L' | 'R' | 'D' => {}
                _ => {
                    return Err(anyhow!(
                        "OpenResearch can't save ssh option -{flag}; remove it and try again."
                    ))
                }
            }
            break;
        }
    }
    let destination = destination.ok_or_else(|| anyhow!("Add the host to connect to."))?;

    let (user, host, port) = split_destination(&destination, command)?;
    if let Some(port) = port {
        options.insert(destination_at, ("Port".into(), port.to_string()));
    }
    if let Some(user) = user {
        options.insert(destination_at, ("User".into(), config_value(&user)));
    }
    for (key, value) in &options {
        if value.is_empty() || value.chars().any(char::is_control) {
            return Err(anyhow!(
                "The ssh option {key} has an empty or invalid value."
            ));
        }
    }
    // The first value wins in ssh, so later repeats would never apply.
    let mut seen = std::collections::HashSet::new();
    options.retain(|(key, _)| {
        let key = key.to_ascii_lowercase();
        key == "identityfile" || seen.insert(key)
    });
    Ok(Block { host, options })
}

/// A Windows shell keeps backslashes (`C:\keys\a`, `CORP\clo`) and only `\"`
/// escapes, so each other backslash is doubled for the POSIX split.
#[cfg_attr(not(windows), allow(dead_code))]
fn literal_backslashes(input: &str) -> String {
    let mut kept = String::new();
    let mut quote = None;
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        kept.push(c);
        match c {
            '\\' if chars.peek() == Some(&'"') => kept.push(chars.next().unwrap()),
            '\\' if quote != Some('\'') => kept.push('\\'),
            '\'' | '"' if quote.is_none() => quote = Some(c),
            c if quote == Some(c) => quote = None,
            _ => {}
        }
    }
    kept
}

fn config_value(value: &str) -> String {
    if value.contains(|c: char| c.is_whitespace() || "\"'#\\".contains(c)) {
        config_quote(value)
    } else {
        value.to_owned()
    }
}

fn flag_option(flag: char) -> Option<(&'static str, &'static str)> {
    Some(match flag {
        '4' => ("AddressFamily", "inet"),
        '6' => ("AddressFamily", "inet6"),
        'A' => ("ForwardAgent", "yes"),
        'a' => ("ForwardAgent", "no"),
        'C' => ("Compression", "yes"),
        'K' => ("GSSAPIAuthentication", "yes"),
        'k' => ("GSSAPIDelegateCredentials", "no"),
        'X' => ("ForwardX11", "yes"),
        'x' => ("ForwardX11", "no"),
        'Y' => ("ForwardX11Trusted", "yes"),
        _ => return None,
    })
}

/// An `-o` option as a config line; options that only shape one session are dropped.
fn config_option(option: &str) -> Result<Option<(String, String)>> {
    let option = option.trim();
    let split = option
        .find(|c: char| c == '=' || c.is_whitespace())
        .ok_or_else(|| anyhow!("The ssh option -o {option} has no value."))?;
    let (key, value) = option.split_at(split);
    let value = value
        .trim_start_matches(|c: char| c.is_whitespace())
        .trim_start_matches('=')
        .trim();
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(anyhow!("The ssh option -o {option} is not valid."));
    }
    let lower = key.to_ascii_lowercase();
    if matches!(lower.as_str(), "host" | "match" | "include") {
        return Err(anyhow!(
            "OpenResearch can't save ssh option {key}; remove it and try again."
        ));
    }
    if matches!(
        lower.as_str(),
        "localforward"
            | "remoteforward"
            | "dynamicforward"
            | "remotecommand"
            | "requesttty"
            | "sessiontype"
            | "stdinnull"
            | "forkafterauthentication"
            | "exitonforwardfailure"
            | "clearallforwardings"
            | "controlmaster"
            | "controlpath"
            | "controlpersist"
            | "localcommand"
            | "permitlocalcommand"
            | "escapechar"
            | "tunnel"
            | "tunneldevice"
    ) {
        return Ok(None);
    }
    if config_args(value).is_empty() {
        return Err(anyhow!("The ssh option -o {key} has no value."));
    }
    Ok(Some((key.to_owned(), value.to_owned())))
}

fn port(value: &str) -> Result<u16> {
    value
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or_else(|| anyhow!("{value} is not a valid port."))
}

/// `[user@]host`, `ssh://[user@]host[:port]`, and without a leading `ssh`,
/// `[user@]host:port` as `orx up --remote` accepts it.
fn split_destination(
    destination: &str,
    command: bool,
) -> Result<(Option<String>, String, Option<u16>)> {
    let (destination, uri) = match destination.strip_prefix("ssh://") {
        Some(rest) => (rest.trim_end_matches('/'), true),
        None => (destination, false),
    };
    let (user, address) = match destination.rsplit_once('@') {
        Some((user, address)) => (Some(user.to_owned()), address),
        None => (None, destination),
    };
    let (host, port) = if let Some(bracketed) = address.strip_prefix('[') {
        let (host, rest) = bracketed
            .split_once(']')
            .ok_or_else(|| anyhow!("{address} is not a valid host."))?;
        let port = match rest.strip_prefix(':') {
            Some(value) if uri || !command => Some(port(value)?),
            None if rest.is_empty() => None,
            _ => return Err(anyhow!("{address} is not a valid host.")),
        };
        (host, port)
    } else {
        match address.split_once(':') {
            Some((host, value))
                if (uri || !command) && !value.contains(':') && !host.is_empty() =>
            {
                (host, Some(port(value)?))
            }
            _ => (address, None),
        }
    };
    if host.is_empty()
        || host.starts_with('-')
        || host
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "*?!,#\"'=@".contains(c))
    {
        return Err(anyhow!("{host} is not a valid host."));
    }
    if let Some(user) = &user {
        if user.is_empty() || user.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(anyhow!("{user} is not a valid user name."));
        }
    }
    Ok((user, host.to_owned(), port))
}

/// The `Host` blocks of a saved list, in file order. A missing file is an empty list.
pub(crate) fn read(list: List) -> Result<Vec<Block>> {
    let path = list.path();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(anyhow!("Could not read {}: {error}", path.display())),
    };
    Ok(blocks(&text))
}

fn blocks(text: &str) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut open = false;
    for line in text.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once([' ', '\t', '=']) else {
            continue;
        };
        if line.starts_with('#') {
            continue;
        }
        let value = value.trim().trim_start_matches('=').trim();
        let args = config_args(value);
        match key.to_ascii_lowercase().as_str() {
            "host" => {
                open = args.len() == 1;
                if open {
                    blocks.push(Block {
                        host: args[0].clone(),
                        options: Vec::new(),
                    });
                }
            }
            "match" => open = false,
            _ if open => {
                if let Some(block) = blocks.last_mut() {
                    block.options.push((key.to_owned(), value.to_owned()));
                }
            }
            _ => {}
        }
    }
    blocks
}

/// Saves `block` to `list`. The same host saved again is a no-op; an alias
/// that already means something else is refused with the user's message.
pub(crate) fn add(list: List, block: &Block) -> Result<std::result::Result<(), String>> {
    let _lock = lock()?;
    let mut block = block.clone();
    for (saved_in, existing) in [(list, read(list)?), (list.other(), read(list.other())?)] {
        if let Some(existing) = existing.iter().find(|saved| saved.host == block.host) {
            // A bare alias names the saved block, which is copied so each list stands alone.
            if existing.same_as(&block) || block.options.is_empty() {
                if saved_in == list {
                    return Ok(Ok(()));
                }
                block = existing.clone();
                continue;
            }
            if saved_in != list && existing.options.is_empty() {
                continue;
            }
            return Ok(Err(format!(
                "{} is already saved {} with different settings. Remove it first.",
                block.host,
                match saved_in {
                    List::Remotes => "as a remote",
                    List::Compute => "as a compute host",
                }
            )));
        }
    }
    let path = list.path();
    let mut text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(anyhow!("Could not read {}: {error}", path.display())),
    };
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&block.render());
    write(&path, &text)?;
    Ok(Ok(()))
}

/// Removes `host` from `list`, false when it wasn't saved there. Other blocks,
/// comments and spacing stay as written.
pub(crate) fn remove(list: List, host: &str) -> Result<bool> {
    let _lock = lock()?;
    let path = list.path();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(anyhow!("Could not read {}: {error}", path.display())),
    };
    let mut kept = String::new();
    let mut found = false;
    let mut dropping = false;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        if let Some((key, value)) = trimmed.split_once([' ', '\t', '=']) {
            if key.eq_ignore_ascii_case("host") || key.eq_ignore_ascii_case("match") {
                let args = config_args(value.trim().trim_start_matches('=').trim());
                dropping = key.eq_ignore_ascii_case("host") && args == [host];
                found |= dropping;
            }
        }
        if !dropping {
            kept.push_str(line);
        }
    }
    if !found {
        return Ok(false);
    }
    write(&path, &kept)?;
    Ok(true)
}

fn write(path: &Path, text: &str) -> Result<()> {
    crate::local::git::atomic_write_with_mode(path, text.as_bytes(), Some(0o600))
        .map_err(|error| anyhow!("Could not save {}: {error}", path.display()))
}

fn lock() -> Result<crate::file_lock::FileLock> {
    let directory = directory();
    std::fs::create_dir_all(&directory)
        .map_err(|error| anyhow!("Could not create {}: {error}", directory.display()))?;
    let path = directory.join(".lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|error| anyhow!("Could not open {}: {error}", path.display()))?;
    file.lock()
        .map_err(|error| anyhow!("Could not lock {}: {error}", path.display()))?;
    Ok(crate::file_lock::FileLock::new(file))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(input: &str) -> Vec<(String, String)> {
        parse(input).unwrap().options
    }

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn a_pasted_command_becomes_the_block_ssh_would_use() {
        let block =
            parse("ssh -p 2222 -J bastion -i '~/keys/lab key' -A clo@gpu1.lab.edu nvidia-smi")
                .unwrap();
        assert_eq!(block.host, "gpu1.lab.edu");
        assert_eq!(
            block.options,
            pairs(&[
                ("Port", "2222"),
                ("ProxyJump", "bastion"),
                ("IdentityFile", "\"~/keys/lab key\""),
                ("ForwardAgent", "yes"),
                ("User", "clo"),
            ])
        );
        assert_eq!(
            block.render(),
            "Host gpu1.lab.edu\n  Port 2222\n  ProxyJump bastion\n  IdentityFile \"~/keys/lab key\"\n  ForwardAgent yes\n  User clo\n"
        );
    }

    #[test]
    fn options_keep_the_text_ssh_reads_and_the_first_user_wins() {
        assert_eq!(
            options("ssh -o 'ProxyCommand=ssh -W %h:%p bastion' -o 'SendEnv LANG LC_*' lab"),
            pairs(&[
                ("ProxyCommand", "ssh -W %h:%p bastion"),
                ("SendEnv", "LANG LC_*"),
            ])
        );
        assert_eq!(options("ssh clo@gpu1 -l bob"), pairs(&[("User", "clo")]));
        assert_eq!(options("ssh -l bob clo@gpu1"), pairs(&[("User", "bob")]));
    }

    #[test]
    fn flags_combine_and_take_attached_values() {
        assert_eq!(
            options("ssh -tt -Ap2222 -lalice -oServerAliveInterval=30 -o 'User bob' lab"),
            pairs(&[
                ("ForwardAgent", "yes"),
                ("Port", "2222"),
                ("User", "alice"),
                ("ServerAliveInterval", "30"),
            ])
        );
    }

    #[test]
    fn windows_backslashes_stay_literal() {
        let words = shell_words::split(&literal_backslashes(
            r#"ssh -i C:\keys\a -i "C:\Program Files\k" -i 'C:\x\y' CORP\clo@h "say \"hi\"""#,
        ))
        .unwrap();
        assert_eq!(
            words,
            [
                "ssh",
                "-i",
                r"C:\keys\a",
                "-i",
                r"C:\Program Files\k",
                "-i",
                r"C:\x\y",
                r"CORP\clo@h",
                r#"say "hi""#,
            ]
        );
    }

    #[test]
    fn session_only_options_are_dropped() {
        assert_eq!(
            options("ssh -L 8888:localhost:8888 -N -v -o RequestTTY=yes -o ControlPath=none lab"),
            Vec::<(String, String)>::new()
        );
    }

    #[test]
    fn destinations_without_ssh_accept_a_port_like_orx_up_remote() {
        let block = parse("root@203.0.113.7:38455").unwrap();
        assert_eq!(block.host, "203.0.113.7");
        assert_eq!(block.options, pairs(&[("User", "root"), ("Port", "38455")]));
        let block = parse("ssh://me@[2001:db8::1]:2200/").unwrap();
        assert_eq!(block.host, "2001:db8::1");
        assert_eq!(block.options, pairs(&[("User", "me"), ("Port", "2200")]));
        assert_eq!(parse("2001:db8::1").unwrap().host, "2001:db8::1");
        assert_eq!(
            parse("lab").unwrap(),
            Block {
                host: "lab".into(),
                options: Vec::new()
            }
        );
    }

    #[test]
    fn unusable_input_names_what_is_wrong() {
        for (input, message) in [
            ("", "Enter a hostname"),
            ("ssh -p", "-p needs a value"),
            ("ssh -p 0 lab", "0 is not a valid port"),
            ("ssh -F other lab", "can't save ssh option -F"),
            ("ssh -Z lab", "Unknown ssh option -Z"),
            ("ssh -o 'Include x' lab", "can't save ssh option Include"),
            ("ssh 'lab*'", "is not a valid host"),
            ("ssh -p 22", "Add the host"),
            ("ssh 'unclosed", "check its quotes"),
            ("ssh -i key.pem ec2-user@lab", "full path to key.pem"),
        ] {
            let error = parse(input).unwrap_err().to_string();
            assert!(error.contains(message), "{input}: {error}");
        }
    }

    #[test]
    fn saved_hosts_are_added_once_listed_and_removed() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        {
            let add = |list, input| add(list, &parse(input).unwrap()).unwrap();
            add(List::Remotes, "ssh -p 2222 clo@gpu1").unwrap();
            add(List::Remotes, "ssh clo@gpu1 -p 2222").unwrap();
            add(List::Remotes, "lab").unwrap();
            assert_eq!(
                read(List::Remotes).unwrap(),
                vec![
                    Block {
                        host: "gpu1".into(),
                        options: pairs(&[("Port", "2222"), ("User", "clo")])
                    },
                    Block {
                        host: "lab".into(),
                        options: Vec::new()
                    },
                ]
            );
            let error = add(List::Compute, "ssh bob@gpu1").unwrap_err();
            assert!(error.contains("already saved as a remote"), "{error}");
            add(List::Compute, "ssh -p 2222 clo@gpu1").unwrap();
            add(List::Compute, "lab").unwrap();
            add(List::Compute, "ssh -J bastion lab9").unwrap();
            add(List::Remotes, "lab9").unwrap();
            assert_eq!(
                read(List::Remotes).unwrap()[2],
                read(List::Compute).unwrap()[2]
            );
            add(List::Remotes, "gpu1").unwrap();
            add(List::Remotes, "ssh -p 2200 gpu1").unwrap_err();

            assert!(remove(List::Remotes, "gpu1").unwrap());
            assert_eq!(
                std::fs::read_to_string(List::Remotes.path()).unwrap(),
                "Host lab\nHost lab9\n  ProxyJump bastion\n"
            );
            assert!(!remove(List::Remotes, "gpu1").unwrap());
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_generated_config_resolves_saved_hosts_then_the_users_own() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        {
            let home = crate::config::home_dir().unwrap();
            std::fs::create_dir_all(home.join(".ssh")).unwrap();
            std::fs::write(
                home.join(".ssh/config"),
                "ServerAliveInterval 7\nHost lab\n  HostName lab.invalid\n  User alice\n",
            )
            .unwrap();
            add(
                List::Remotes,
                &parse("ssh -p 2222 clo@gpu1.invalid").unwrap(),
            )
            .unwrap()
            .unwrap();
            add(List::Compute, &parse("ssh -p 2200 lab").unwrap())
                .unwrap()
                .unwrap();
            let config = config_file(false).unwrap();
            let resolve = |host: &str| {
                let output = crate::process::command("ssh")
                    .arg("-F")
                    .arg(&config)
                    .args(["-G", host])
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                String::from_utf8(output.stdout).unwrap()
            };
            let gpu = resolve("gpu1.invalid");
            for line in [
                "user clo",
                "port 2222",
                "hostname gpu1.invalid",
                "serveraliveinterval 7",
            ] {
                assert!(gpu.lines().any(|l| l == line), "{line}\n{gpu}");
            }
            let lab = resolve("lab");
            for line in ["user alice", "port 2200", "hostname lab.invalid"] {
                assert!(lab.lines().any(|l| l == line), "{line}\n{lab}");
            }
        }
    }
}
