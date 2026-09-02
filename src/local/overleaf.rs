//! Keep a paper and an Overleaf project in step.
//!
//! Local-only, like `crate::local::latex`: `orx up` runs on the user's machine,
//! so the API process drives their own `git` against Overleaf's git bridge.
//!
//! Two paths, because Overleaf gives us two:
//!
//!   - **Git** — a real push into an existing project
//!     (`https://git.overleaf.com/<id>`, username `git` plus a Git
//!     authentication token). This is a premium Overleaf feature, and there is
//!     no endpoint that reports whether an account has it: the only way to know
//!     is to talk to the bridge and read what it says back, which is what
//!     `classify` is for.
//!   - **Upload** — a form POST to `https://www.overleaf.com/docs`, which every
//!     account can do. It creates a *new* project rather than updating one, so
//!     it is the fallback, not the default.
//!
//! The bridge cannot create projects, so a paper is linked to a project the
//! user already owns; `crate::store` remembers which.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output};

use base64::Engine as _;

use crate::error::{anyhow, Result};
use crate::local::git::TemporaryDirectory;

/// The name the credential helper reads the token from. Only ever the *name*
/// reaches git's configuration; the value stays in the child's environment.
const TOKEN_ENV: &str = "ORX_OVERLEAF_TOKEN";

/// The host that helper will answer for, so a project URL naming somewhere else
/// cannot collect the token.
const HOST_ENV: &str = "ORX_OVERLEAF_HOST";

/// What a paper is made of. A pull writes only these: the Overleaf project is a
/// co-author's to edit, and it syncs into a checkout that orx also runs
/// experiments out of — a `.yml` or a `Makefile` arriving from there would be
/// picked up by something other than LaTeX.
const PULLABLE_EXTENSIONS: &[&str] = &[
    "tex", "bib", "cls", "sty", "bst", "png", "jpg", "jpeg", "pdf", "eps", "svg",
];

/// Overleaf caps a project at far more than this; the limit here is about not
/// mistaking a checkout for a paper when a `\includegraphics` path is wrong.
const MAX_FILES: usize = 64;
const MAX_TOTAL_BYTES: u64 = 20 * 1024 * 1024;

/// The upload fallback inlines every file as a base64 data URL inside one form
/// POST, which is a much tighter budget than a git push.
const MAX_UPLOAD_BYTES: u64 = 8 * 1024 * 1024;

/// Tail of git's stderr handed back when a push fails for a reason we cannot
/// name. Enough for the real message, short of a whole transfer log.
const STDERR_TAIL_BYTES: usize = 2 * 1024;

pub fn token() -> Option<String> {
    crate::config::overleaf_token()
}

pub fn set_token(token: &str) -> Result<()> {
    crate::config::set_overleaf_token(token)
}

pub fn clear_token() -> Result<()> {
    crate::config::clear_overleaf_token()
}

/// An Overleaf project, identified the way the git bridge identifies one.
#[derive(Debug, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    /// The site it lives on: `www.overleaf.com`, or a Server Pro host.
    pub host: String,
}

impl Project {
    fn is_cloud(&self) -> bool {
        self.host == "www.overleaf.com" || self.host == "overleaf.com"
    }

    /// Cloud puts the bridge on its own host; Server Pro serves it from the
    /// site under `/git`.
    pub fn git_url(&self) -> String {
        if self.is_cloud() {
            format!("https://git.overleaf.com/{}", self.id)
        } else {
            format!("https://{}/git/{}", self.host, self.id)
        }
    }

    /// The host git actually talks to, which is what the credential helper is
    /// scoped to — not `host`, which is where the project is *read*.
    fn git_host(&self) -> String {
        if self.is_cloud() {
            "git.overleaf.com".to_string()
        } else {
            self.host.clone()
        }
    }

    pub fn web_url(&self) -> String {
        let host = if self.is_cloud() {
            "www.overleaf.com"
        } else {
            &self.host
        };
        format!("https://{host}/project/{}", self.id)
    }
}

/// Read a project out of whatever the user pasted: the URL from their browser,
/// the clone URL, or the bare id.
pub fn parse_project(input: &str) -> Result<Project> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err(anyhow!("Paste an Overleaf project URL."));
    }
    let Some((host, segments)) = split_url(raw) else {
        return valid_id(raw)
            .map(|id| Project {
                id,
                host: "www.overleaf.com".to_string(),
            })
            .ok_or_else(bad_project);
    };
    // `…/project/<id>` on the site, `git.overleaf.com/<id>` on the bridge.
    let host = host.as_str();
    let id = match segments.iter().position(|s| *s == "project") {
        Some(at) => segments.get(at + 1).copied(),
        None if host.starts_with("git.") || segments.first() == Some(&"git") => {
            segments.iter().find(|s| valid_id(s).is_some()).copied()
        }
        None => None,
    };
    let id = id.and_then(valid_id).ok_or_else(bad_project)?;
    let host = match host.strip_prefix("git.") {
        Some(site) if site.ends_with("overleaf.com") => "www.overleaf.com".to_string(),
        Some(site) => site.to_string(),
        None => host.to_string(),
    };
    Ok(Project { id, host })
}

fn bad_project() -> crate::error::Error {
    anyhow!("That does not look like an Overleaf project. Open the project in Overleaf and copy the URL from the address bar — it looks like https://www.overleaf.com/project/64f0c1a2b3d4e5f60718293a")
}

/// Host plus path segments, or None when this is not a URL at all.
fn split_url(raw: &str) -> Option<(String, Vec<&str>)> {
    let rest = raw
        .split_once("://")
        .map(|(_, rest)| rest)
        .or_else(|| raw.starts_with("www.").then_some(raw))?;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    // A pasted URL can carry any casing; the port stays, because a Server Pro
    // instance can be on one and git sends it to the credential helper as part
    // of `host=`.
    let host = host.to_ascii_lowercase();
    let path = path.split(['?', '#']).next().unwrap_or(path);
    Some((host, path.split('/').filter(|s| !s.is_empty()).collect()))
}

/// Overleaf ids are hex object ids, but read tokens and Server Pro ids differ
/// enough that shape, not format, is what we can insist on.
fn valid_id(candidate: &str) -> Option<String> {
    let id = candidate.trim();
    let ok = (6..=64).contains(&id.len())
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && id.chars().any(|c| c.is_ascii_alphanumeric());
    ok.then(|| id.to_string())
}

/// The paper and everything it needs to compile somewhere else.
pub struct Payload {
    /// The paper's directory — where a pulled file is written, and the boundary
    /// nothing may be written outside of.
    pub dir: PathBuf,
    /// Name of the `.tex` inside the project, e.g. `paper.tex`.
    pub main: String,
    /// Project-relative path to absolute path, for every file that travels.
    pub files: BTreeMap<String, PathBuf>,
    /// Referenced but not sent — missing, or past the budget. Shown as a note.
    pub skipped: Vec<String>,
}

impl Payload {
    pub fn total_bytes(&self) -> u64 {
        self.files
            .values()
            .filter_map(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .sum()
    }
}

/// Commands whose braces name a file, with the extensions LaTeX would try when
/// the reference leaves one off.
const REFERENCE_COMMANDS: &[(&str, &[&str])] = &[
    (
        "includegraphics",
        &[".pdf", ".png", ".jpg", ".jpeg", ".eps", ".svg"],
    ),
    ("input", &[".tex"]),
    ("include", &[".tex"]),
    ("bibliography", &[".bib"]),
    ("addbibresource", &[".bib"]),
];

/// Extensions of the support files a paper directory carries: the class and
/// style files a conference template ships, and its bibliography.
const SUPPORT_EXTENSIONS: &[&str] = &["cls", "sty", "bst", "bib"];

/// Gather the `.tex` and the files it references. Confined to the directory the
/// `.tex` sits in: a paper lives beside its figures, and a reference that walks
/// out of that directory would send parts of the checkout to Overleaf.
pub fn collect(tex: &Path) -> Result<Payload> {
    let dir = tex
        .parent()
        .ok_or_else(|| anyhow!("the paper has no parent directory"))?;
    let root = std::fs::canonicalize(dir)?;
    let main = tex
        .file_name()
        .ok_or_else(|| anyhow!("the paper has no file name"))?
        .to_string_lossy()
        .to_string();

    let mut files = BTreeMap::new();
    let mut skipped = Vec::new();
    files.insert(main.clone(), tex.to_path_buf());
    let mut total = std::fs::metadata(tex)?.len();

    // A paper is usually more than one file: `\input{sections/method}` pulls in
    // a source with figures and citations of its own, and scanning only the
    // entry point would send a `.tex` tree with none of them.
    let mut sources = vec![tex.to_path_buf()];
    let mut scanned: BTreeSet<PathBuf> = BTreeSet::new();
    let mut pending: Vec<Vec<String>> = support_files(&root)
        .into_iter()
        .map(|name| vec![name])
        .collect();

    while !sources.is_empty() || !pending.is_empty() {
        while let Some(path) = sources.pop() {
            if !scanned.insert(path.clone()) {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&path) {
                pending.extend(scan_references(&text));
            }
        }
        for reference in std::mem::take(&mut pending) {
            // `\bibliography{refs}` names one file under several possible
            // spellings; it is missing only when none of them is there.
            let Some((rel, path)) = reference.iter().find_map(|c| resolve(&root, c)) else {
                skipped.push(reference[0].clone());
                continue;
            };
            if files.contains_key(&rel) {
                continue;
            }
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if files.len() >= MAX_FILES || total + size > MAX_TOTAL_BYTES {
                skipped.push(rel);
                continue;
            }
            total += size;
            if rel.to_ascii_lowercase().ends_with(".tex") {
                sources.push(path.clone());
            }
            files.insert(rel, path);
        }
    }
    Ok(Payload {
        dir: root,
        main,
        files,
        skipped,
    })
}

/// A reference resolved to a real file inside `root`, as (project-relative
/// path, absolute path). None when it does not exist or escapes the directory.
fn resolve(root: &Path, reference: &str) -> Option<(String, PathBuf)> {
    let trimmed = reference.trim().trim_matches('"');
    if trimmed.is_empty() || Path::new(trimmed).is_absolute() {
        return None;
    }
    // A dotted path is one the pull will refuse to write back, so sending it
    // would leave a file that can never be reconciled.
    if trimmed.split('/').any(|part| part.starts_with('.')) {
        return None;
    }
    let path = std::fs::canonicalize(root.join(trimmed)).ok()?;
    if !path.is_file() || !path.starts_with(root) {
        return None;
    }
    let rel = path
        .strip_prefix(root)
        .ok()?
        .to_string_lossy()
        .replace('\\', "/");
    Some((rel, path))
}

/// Class, style and bibliography files sitting beside the paper — what the
/// paper skill copies out of an uploaded template. Not recursive: a template's
/// files land next to the `.tex`.
fn support_files(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| SUPPORT_EXTENSIONS.contains(&x.to_ascii_lowercase().as_str()))
        })
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect()
}

/// Every path named by a `REFERENCE_COMMANDS` command, one group per reference:
/// the path as written, then the extensions a bare reference could mean.
fn scan_references(source: &str) -> Vec<Vec<String>> {
    // Comments go first, per line, so a commented-out figure is not sent; the
    // scan then runs across the whole document, because a real paper wraps
    // `\includegraphics[width=...]{...}` over two lines often enough.
    let source: String = source
        .lines()
        .map(strip_comment)
        .collect::<Vec<_>>()
        .join("\n");
    let mut out = Vec::new();
    let mut rest = source.as_str();
    while let Some(at) = rest.find('\\') {
        rest = &rest[at + 1..];
        let name_len = rest
            .char_indices()
            .find(|(_, c)| !c.is_ascii_alphabetic())
            .map_or(rest.len(), |(i, _)| i);
        let (name, tail) = rest.split_at(name_len);
        let Some((_, extensions)) = REFERENCE_COMMANDS.iter().find(|(c, _)| *c == name) else {
            continue;
        };
        let Some((argument, after)) = braced_argument(tail) else {
            continue;
        };
        for reference in argument.split(',') {
            push_candidates(&mut out, reference, extensions);
        }
        rest = after;
    }
    out
}

fn push_candidates(out: &mut Vec<Vec<String>>, reference: &str, extensions: &[&str]) {
    let reference = reference.trim();
    if reference.is_empty() {
        return;
    }
    let mut group = vec![reference.to_string()];
    // `\includegraphics{fig/loss}` and `\bibliography{refs}` leave the
    // extension to LaTeX, so we try the same ones it would.
    let has_extension = Path::new(reference).extension().is_some_and(|x| {
        extensions
            .iter()
            .any(|e| e[1..].eq_ignore_ascii_case(&x.to_string_lossy()))
    });
    if !has_extension {
        group.extend(extensions.iter().map(|e| format!("{reference}{e}")));
    }
    out.push(group);
}

/// Contents of the `{…}` that follows, skipping an optional `[…]`, plus what
/// comes after it.
fn braced_argument(tail: &str) -> Option<(&str, &str)> {
    let tail = tail.strip_prefix('*').unwrap_or(tail).trim_start();
    let tail = match tail.strip_prefix('[') {
        Some(rest) => rest.split_once(']')?.1.trim_start(),
        None => tail,
    };
    tail.strip_prefix('{')?.split_once('}')
}

/// Everything from an unescaped `%` on is a comment.
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'%' => return &line[..i],
            _ => i += 1,
        }
    }
    line
}

/// Why the bridge said no. The difference matters: a plan without git
/// integration is a dead end we route around, a bad token is one the user
/// fixes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Refusal {
    /// The account cannot use the git bridge at all.
    Unsupported,
    /// The token is missing, wrong, or expired.
    Auth,
    /// The project id does not name a project this account can reach.
    NotFound,
}

impl Refusal {
    fn message(self) -> &'static str {
        match self {
            Refusal::Unsupported => "This Overleaf account does not have Git integration — it comes with a paid Overleaf plan. You can still upload a copy as a new project.",
            Refusal::Auth => "Overleaf rejected the Git authentication token. Generate one under Account Settings in Overleaf — and note that Git integration itself needs a paid Overleaf plan.",
            Refusal::NotFound => "Overleaf has no project with that id, or this account cannot open it.",
        }
    }
}

/// Read git's stderr for a refusal we can act on. Matched on the phrases git
/// actually prints rather than on a bare status code: git echoes the failing
/// URL, and an Overleaf project id is 24 hex characters that can contain `403`
/// by chance.
fn classify(stderr: &str) -> Option<Refusal> {
    let text = stderr.to_ascii_lowercase();
    let status = |code: &str| text.contains(&format!("returned error: {code}"));
    if status("401") || text.contains("authentication failed") {
        return Some(Refusal::Auth);
    }
    if status("403") || text.contains("premium") || text.contains("subscription") {
        return Some(Refusal::Unsupported);
    }
    if status("404") || (text.contains("repository") && text.contains("not found")) {
        return Some(Refusal::NotFound);
    }
    None
}

/// Credentials for one Overleaf host. Absent for a repository that needs none,
/// which is every command run inside the clone.
#[derive(Clone, Copy)]
struct Auth<'a> {
    host: &'a str,
    token: &'a str,
}

/// Answers only for the host the project was linked with: git feeds the helper
/// the host on stdin, and a project URL naming somewhere else gets nothing.
const CREDENTIAL_HELPER: &str = "!f() { host=; while IFS='=' read key value; do [ \"$key\" = host ] && host=$value; done; [ \"$host\" = \"$ORX_OVERLEAF_HOST\" ] || exit 0; echo username=git; echo \"password=$ORX_OVERLEAF_TOKEN\"; }; f";

/// A git run. The token reaches the child only through its environment — never
/// argv, never the clone's config — and only the variable's *name* appears in
/// the configuration git carries into `git-remote-https`.
fn git(dir: Option<&Path>, auth: Option<Auth>, args: &[&str]) -> Result<Output> {
    let mut command = Command::new("git");
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    command.env("GIT_TERMINAL_PROMPT", "0");
    if let Some(auth) = auth {
        command
            .env(HOST_ENV, auth.host)
            .env(TOKEN_ENV, auth.token)
            // The empty value first clears helpers configured system- and
            // user-wide, so a keychain helper cannot answer instead.
            .env("GIT_CONFIG_COUNT", "4")
            .env("GIT_CONFIG_KEY_0", "credential.helper")
            .env("GIT_CONFIG_VALUE_0", "")
            .env("GIT_CONFIG_KEY_1", "credential.helper")
            .env("GIT_CONFIG_VALUE_1", CREDENTIAL_HELPER)
            // A hung transfer would otherwise hold a blocking thread forever,
            // and the poll starts a new one every thirty seconds.
            .env("GIT_CONFIG_KEY_2", "http.lowSpeedLimit")
            .env("GIT_CONFIG_VALUE_2", "1000")
            .env("GIT_CONFIG_KEY_3", "http.lowSpeedTime")
            .env("GIT_CONFIG_VALUE_3", "30");
    }
    command
        .args(args)
        .output()
        .map_err(|e| anyhow!("Could not run git: {e}"))
}

fn refuse(output: &Output) -> crate::error::Error {
    let stderr = String::from_utf8_lossy(&output.stderr);
    match classify(&stderr) {
        Some(refusal) => anyhow!("{}", refusal.message()),
        None => anyhow!("{}", tail(stderr.trim())),
    }
}

fn tail(text: &str) -> String {
    if text.len() <= STDERR_TAIL_BYTES {
        return text.to_string();
    }
    let start = text.len() - STDERR_TAIL_BYTES;
    let start = (start..text.len())
        .find(|i| text.is_char_boundary(*i))
        .unwrap_or(text.len());
    format!("…{}", &text[start..])
}

/// Ask the bridge whether this account can reach this project. The one probe
/// that tells us whether git integration is available at all.
pub fn probe(project: &Project, token: &str) -> Result<()> {
    remote_head(project, token).map(|_| ())
}

/// What both sides agreed on at the last sync: project-relative path to a hash
/// of the content each had. Without it a difference between Overleaf and the
/// checkout has no direction — this is what says whether Overleaf moved, we
/// moved, or both did.
pub type Baseline = BTreeMap<String, String>;

/// How the user settled a file both sides changed. A conflict is never resolved
/// automatically; this carries the answer they gave.
#[derive(Clone, Copy, Debug)]
pub enum Resolution {
    /// Overwrite the checkout with Overleaf's copy.
    TakeOverleaf,
    /// Push over Overleaf with the checkout's copy.
    KeepLocal,
}

pub struct SyncOutcome {
    /// Files Overleaf changed alone, now written into the checkout.
    pub pulled: Vec<String>,
    /// Files we changed alone, now committed to Overleaf.
    pub pushed: Vec<String>,
    /// Files both sides changed. Left untouched on both sides — resolving one
    /// into the other would silently discard somebody's writing.
    pub conflicts: Vec<String>,
    /// Anything else worth saying: a main-document mismatch, files left behind.
    pub note: Option<String>,
    /// Overleaf's HEAD when the sync finished, for the next poll to compare.
    pub head: String,
    /// The agreement to carry into the next sync.
    pub baseline: Baseline,
}

/// Overleaf's current HEAD, without cloning. One request, so a linked paper can
/// be watched cheaply and the clone kept for when something actually moved.
pub fn remote_head(project: &Project, token: &str) -> Result<String> {
    let host = project.git_host();
    let auth = Auth { host: &host, token };
    let output = git(None, Some(auth), &["ls-remote", &project.git_url(), "HEAD"])?;
    if !output.status.success() {
        return Err(refuse(&output));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string())
}

/// Bring the checkout and the Overleaf project into step, in both directions.
///
/// Cloning per sync rather than keeping a mirror is what makes this safe to run
/// on every compile and every poll: the history we commit onto is always
/// Overleaf's current one, so a co-author's edits cannot be clobbered and a
/// push cannot be rejected as non-fast-forward.
pub fn sync(
    payload: &Payload,
    project: &Project,
    token: &str,
    baseline: &Baseline,
    resolutions: &BTreeMap<String, Resolution>,
) -> Result<SyncOutcome> {
    let host = project.git_host();
    let auth = Auth { host: &host, token };
    sync_with(
        payload,
        &project.git_url(),
        Some(auth),
        baseline,
        resolutions,
    )
}

/// The paper's folder relative to the checkout, from the paper's own path —
/// `None` when the paper sits at the checkout root.
pub fn folder_of(checkout_relative_tex: &str) -> Option<String> {
    checkout_relative_tex
        .rsplit_once('/')
        .map(|(dir, _)| dir.to_string())
}

/// The sync speaks in paths relative to the paper's folder, because that is the
/// Overleaf project's root; everything the user sees is relative to the
/// checkout. These translate between the two — getting it wrong would let the
/// dashboard miss that the file it has open is the one just pulled.
pub fn to_checkout(folder: Option<&str>, paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .map(|path| match folder {
            Some(dir) => format!("{dir}/{path}"),
            None => path.clone(),
        })
        .collect()
}

pub fn from_checkout(folder: Option<&str>, path: &str) -> Option<String> {
    match folder {
        Some(dir) => path.strip_prefix(&format!("{dir}/")).map(str::to_string),
        // The paper sits at the checkout root, so the two namespaces coincide —
        // including for a figure in a subdirectory.
        None => Some(path.to_string()),
    }
}

/// Split from `sync` so the whole sequence can be exercised against a local
/// repository, where no Overleaf plan is involved.
fn sync_with(
    payload: &Payload,
    remote: &str,
    auth: Option<Auth>,
    baseline: &Baseline,
    resolutions: &BTreeMap<String, Resolution>,
) -> Result<SyncOutcome> {
    let temporary = TemporaryDirectory::new("orx-overleaf")?;
    let clone = temporary.path().join("project");
    let clone_arg = clone.to_string_lossy().to_string();

    let output = git(
        None,
        auth,
        &[
            "-c",
            "core.hooksPath=",
            // The bytes must round-trip: git-for-windows checks out CRLF by
            // default, and hashing a smudged working tree against the LF file
            // on disk would make every file look changed on both sides.
            "-c",
            "core.autocrlf=false",
            "-c",
            "core.eol=lf",
            "clone",
            "--quiet",
            remote,
            &clone_arg,
        ],
    )?;
    if !output.status.success() {
        return Err(refuse(&output));
    }

    let mut notes = Vec::new();
    let remote_files = tree_files(&clone)?;
    if let Some(note) = main_document_note(&remote_files, &payload.main) {
        notes.push(note);
    }
    if !payload.skipped.is_empty() {
        notes.push(format!(
            "Not sent (missing, outside the paper's folder, or past the size limit): {}.",
            payload.skipped.join(", ")
        ));
    }

    let mut plan = plan(
        &clone,
        &remote_files,
        payload,
        baseline,
        resolutions,
        &mut notes,
    );
    let (staged, pushed_hashes) =
        stage(&clone, &remote_files, payload, baseline, &plan, &mut notes)?;
    plan.baseline.extend(pushed_hashes);
    let (head, committed) = commit_and_push(&clone, payload, auth, &staged)?;

    // The checkout is written only now. A push that fails after we had already
    // replaced the paper on disk would leave the editor showing the old text
    // over new bytes, and the next save would send that stale copy back.
    let pulled = apply(&payload.dir, baseline, &mut plan, &mut notes);

    Ok(SyncOutcome {
        pulled,
        pushed: if committed { staged } else { Vec::new() },
        conflicts: plan.conflicts,
        note: (!notes.is_empty()).then(|| notes.join(" ")),
        head,
        baseline: plan.baseline,
    })
}

/// What a sync intends to do, worked out before anything is written anywhere.
#[derive(Default)]
struct Plan {
    /// Files to write into the checkout, held in memory until the push lands.
    pulls: Vec<(String, PathBuf, Vec<u8>)>,
    conflicts: Vec<String>,
    /// Locally-changed files the user chose to keep which the paper does not
    /// itself reference — `stage` iterates the payload, so it would not
    /// otherwise see them.
    forced: BTreeMap<String, PathBuf>,
    /// Files this sync decided not to act on. Staging must skip them: a file we
    /// declined to pull still differs from its agreement, and pushing on that
    /// difference would send the local copy over the remote change we just
    /// refused — the opposite of what was asked.
    held: BTreeSet<String>,
    baseline: Baseline,
}

/// Decide, for every file Overleaf has, which way it should move. Writes
/// nothing: the agreement in `baseline` is what gives each difference a
/// direction, and a file whose direction is unclear becomes a conflict.
fn plan(
    clone: &Path,
    remote_files: &BTreeSet<String>,
    payload: &Payload,
    baseline: &Baseline,
    resolutions: &BTreeMap<String, Resolution>,
    notes: &mut Vec<String>,
) -> Plan {
    let mut plan = Plan::default();
    let mut deleted_here = Vec::new();
    let mut left_alone = Vec::new();
    let mut refused = Vec::new();
    let mut bytes = 0u64;
    // Carrying the agreement forward is what lets a later sync still tell which
    // side moved; without it, a one-sided edit reads as a conflict.
    let keep = |plan: &mut Plan, rel: &str| {
        if let Some(base) = baseline.get(rel) {
            plan.baseline.insert(rel.to_string(), base.clone());
        }
    };
    let hold = |plan: &mut Plan, rel: &str| {
        keep(plan, rel);
        plan.held.insert(rel.to_string());
    };

    for rel in remote_files {
        let Some(local_path) = confined_path(&payload.dir, rel) else {
            refused.push(rel.clone());
            hold(&mut plan, rel);
            continue;
        };
        let source = clone.join(rel);
        // A submodule, or anything else that is not a plain file.
        let Ok(metadata) = std::fs::symlink_metadata(&source) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let Ok(remote_bytes) = std::fs::read(&source) else {
            continue;
        };
        let remote_hash = hash(&remote_bytes);
        let local_hash = std::fs::read(&local_path).ok().map(|b| hash(&b));
        let base = baseline.get(rel).map(String::as_str);

        if local_hash.as_deref() == Some(remote_hash.as_str()) {
            plan.baseline.insert(rel.clone(), remote_hash);
            continue;
        }
        if !pullable(rel) && !payload.files.contains_key(rel) {
            left_alone.push(rel.clone());
            hold(&mut plan, rel);
            continue;
        }
        let we_moved = local_hash.as_deref() != base;
        let overleaf_moved = base != Some(remote_hash.as_str());

        // Ours alone: the staging below carries it, so the agreement is kept but
        // the file is not held.
        if we_moved && !overleaf_moved {
            if local_hash.is_none() {
                // Deleted here. Deletions are not propagated, and holding the
                // agreement is what stops the next sync pulling it back.
                deleted_here.push(rel.clone());
                hold(&mut plan, rel);
            } else {
                keep(&mut plan, rel);
            }
            continue;
        }
        let take_overleaf = match (we_moved, resolutions.get(rel)) {
            // Overleaf alone.
            (false, _) => true,
            (true, Some(Resolution::TakeOverleaf)) => true,
            (true, Some(Resolution::KeepLocal)) => false,
            // Both, or a first sync where each side already had a different
            // file of that name. Neither copy is safe to discard unasked.
            (true, None) => {
                plan.conflicts.push(rel.clone());
                hold(&mut plan, rel);
                continue;
            }
        };
        if !take_overleaf {
            // The agreement becomes Overleaf's copy, which is what makes the
            // staging below see the local file as a change and push it over.
            plan.baseline.insert(rel.clone(), remote_hash);
            if !payload.files.contains_key(rel) {
                // `confined_path` is lexical; this is the same canonicalized
                // boundary `collect` applies, so a symlinked folder cannot make
                // a "keep this copy" send something from outside the paper.
                match std::fs::canonicalize(&local_path) {
                    Ok(real) if real.starts_with(&payload.dir) => {
                        plan.forced.insert(rel.clone(), real);
                    }
                    // Keeping a copy that is not there means keeping it
                    // deleted, and deletions do not travel.
                    Err(_) if local_hash.is_none() => {
                        deleted_here.push(rel.clone());
                        hold(&mut plan, rel);
                    }
                    _ => {
                        refused.push(rel.clone());
                        hold(&mut plan, rel);
                    }
                }
            }
            continue;
        }
        if plan.pulls.len() >= MAX_FILES || bytes + metadata.len() > MAX_TOTAL_BYTES {
            refused.push(rel.clone());
            hold(&mut plan, rel);
            continue;
        }
        bytes += metadata.len();
        plan.baseline.insert(rel.clone(), remote_hash);
        plan.pulls.push((rel.clone(), local_path, remote_bytes));
    }

    if !deleted_here.is_empty() {
        notes.push(format!(
            "Still on Overleaf, deleted here: {}. Delete them in Overleaf too if that was the intent.",
            deleted_here.join(", ")
        ));
    }
    if !left_alone.is_empty() {
        notes.push(format!(
            "Left on Overleaf, not part of a paper: {}.",
            left_alone.join(", ")
        ));
    }
    if !refused.is_empty() {
        notes.push(format!(
            "Not pulled (unsafe path, or past the size limit): {}.",
            refused.join(", ")
        ));
    }
    plan
}

/// Write the pulled files into the checkout. Runs only after Overleaf accepted
/// the push; a file that cannot be written drops its agreement so the next sync
/// tries again rather than believing the checkout already has it.
fn apply(dir: &Path, baseline: &Baseline, plan: &mut Plan, notes: &mut Vec<String>) -> Vec<String> {
    let mut pulled = Vec::new();
    let mut failed = Vec::new();
    for (rel, path, bytes) in std::mem::take(&mut plan.pulls) {
        match write_pulled(dir, &path, &bytes) {
            Ok(()) => pulled.push(rel),
            Err(_) => {
                match baseline.get(&rel) {
                    Some(base) => plan.baseline.insert(rel.clone(), base.clone()),
                    None => plan.baseline.remove(&rel),
                };
                failed.push(rel);
            }
        }
    }
    if !failed.is_empty() {
        notes.push(format!(
            "Could not write Overleaf's copy of {} — check that nothing in the paper's folder is a symlink or read-only.",
            failed.join(", ")
        ));
    }
    pulled
}

/// Copy the paper's own files into the clone and stage them. Skips whatever the
/// plan already settled, so a push only ever carries what we actually changed.
fn stage(
    clone: &Path,
    remote_files: &BTreeSet<String>,
    payload: &Payload,
    baseline: &Baseline,
    plan: &Plan,
    notes: &mut Vec<String>,
) -> Result<(Vec<String>, Baseline)> {
    let mut staged = Vec::new();
    let mut deleted_there = Vec::new();
    let mut pushed_hashes = Baseline::new();
    let pulling: BTreeSet<&String> = plan.pulls.iter().map(|(rel, _, _)| rel).collect();
    for (rel, source) in payload.files.iter().chain(plan.forced.iter()) {
        if plan.held.contains(rel) || pulling.contains(rel) {
            continue;
        }
        // Gone from Overleaf but still ours, and we agreed on it once: someone
        // deleted it there. Pushing it back would undo that silently — and the
        // agreement has to travel forward, or the next sync sees no deletion to
        // respect and pushes it after all.
        if !remote_files.contains(rel) && baseline.contains_key(rel) {
            deleted_there.push(rel.clone());
            pushed_hashes.insert(rel.clone(), baseline[rel].clone());
            continue;
        }
        // A file the user chose to keep can since have been deleted here;
        // deletions are not propagated, so there is nothing to send.
        let Ok(bytes) = std::fs::read(source) else {
            continue;
        };
        let hashed = hash(&bytes);
        if plan.baseline.get(rel) == Some(&hashed) {
            continue;
        }
        let destination = clone.join(rel);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&destination, &bytes)?;
        staged.push(rel.clone());
        pushed_hashes.insert(rel.clone(), hashed);
    }
    if !deleted_there.is_empty() {
        notes.push(format!(
            "Deleted on Overleaf but still here: {}. Not sent back — delete them here too if that was the intent.",
            deleted_there.join(", ")
        ));
    }
    Ok((staged, pushed_hashes))
}

/// Commit and push the staged files. Reports Overleaf's resulting HEAD, and
/// whether a commit was made at all — a file can stage identical to what the
/// project already had, and that is not a push.
fn commit_and_push(
    clone: &Path,
    payload: &Payload,
    auth: Option<Auth>,
    staged: &[String],
) -> Result<(String, bool)> {
    if staged.is_empty() {
        return Ok((head_sha(clone)?, false));
    }
    // `:(literal)` because a real figure can be named `fig[1].png`, and git
    // would otherwise read the name as a pathspec that matches nothing.
    let literal: Vec<String> = staged
        .iter()
        .map(|rel| format!(":(literal){rel}"))
        .collect();
    let mut add = vec!["-c", "core.autocrlf=false", "add", "--force", "--"];
    add.extend(literal.iter().map(String::as_str));
    let output = git(Some(clone), None, &add)?;
    if !output.status.success() {
        return Err(refuse(&output));
    }

    let unchanged = git(Some(clone), None, &["diff", "--cached", "--quiet"])
        .map(|o| o.status.success())
        .unwrap_or(false);
    if unchanged {
        return Ok((head_sha(clone)?, false));
    }

    let message = format!("Update {} from orx", payload.main);
    // Signing and hooks are the user's setup for their own repositories, and
    // Overleaf's bridge verifies neither; letting them run here only turns a
    // sync into an opaque failure. Same reasoning as `git::create_initial_commit`.
    let mut commit = vec![
        "-c",
        "commit.gpgSign=false",
        "-c",
        "core.hooksPath=",
        "commit",
        "--quiet",
        "--no-verify",
        "--no-gpg-sign",
        "-m",
        &message,
    ];
    // A machine that never configured a git identity cannot commit; only then
    // do we supply one, so the user's own name stays on the commit when it is
    // set.
    if !has_identity(clone) {
        let mut with_identity = vec![
            "-c",
            "user.name=orx",
            "-c",
            "user.email=orx@openresearch.sh",
        ];
        with_identity.append(&mut commit);
        commit = with_identity;
    }
    let output = git(Some(clone), None, &commit)?;
    if !output.status.success() {
        return Err(refuse(&output));
    }

    let branch = git(Some(clone), None, &["symbolic-ref", "--short", "HEAD"])
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| "master".to_string());
    let refspec = format!("HEAD:refs/heads/{branch}");
    let output = git(
        Some(clone),
        auth,
        &["push", "--quiet", "--no-verify", "origin", &refspec],
    )?;
    if !output.status.success() {
        return Err(refuse(&output));
    }
    Ok((head_sha(clone)?, true))
}

fn has_identity(clone: &Path) -> bool {
    git(Some(clone), None, &["var", "GIT_COMMITTER_IDENT"])
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn head_sha(clone: &Path) -> Result<String> {
    Ok(git(Some(clone), None, &["rev-parse", "HEAD"])
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default())
}

/// Every path in the project, NUL-separated so a name with a quote or a
/// non-ASCII character arrives as git stored it.
///
/// An unborn HEAD is an empty project and an empty set; any other failure is
/// raised, because reading it as "empty" would make the sync push every local
/// file over whatever Overleaf actually has.
fn tree_files(clone: &Path) -> Result<BTreeSet<String>> {
    if !git(
        Some(clone),
        None,
        &["rev-parse", "--verify", "--quiet", "HEAD"],
    )
    .map(|o| o.status.success())
    .unwrap_or(false)
    {
        return Ok(BTreeSet::new());
    }
    let output = git(
        Some(clone),
        None,
        &["ls-tree", "-r", "-z", "--name-only", "HEAD"],
    )?;
    if !output.status.success() {
        return Err(refuse(&output));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect())
}

/// A remote path resolved under the paper's directory, or None when it does not
/// stay there. Overleaf paths are ordinary relative paths, so this only ever
/// refuses something that should not have arrived.
fn confined_path(dir: &Path, rel: &str) -> Option<PathBuf> {
    let path = Path::new(rel);
    let ordinary = path.components().all(|c| match c {
        // A dotted component is how a config or workflow file would arrive.
        Component::Normal(name) => !name.to_string_lossy().starts_with('.'),
        _ => false,
    });
    (!path.is_absolute() && ordinary).then(|| dir.join(path))
}

fn pullable(rel: &str) -> bool {
    Path::new(rel)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| PULLABLE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Never follow a symlink out of the paper's directory: the pull writes files,
/// not wherever a link in the checkout happens to point. Every directory on the
/// way is checked, not just the leaf, since a symlinked `figs/` would carry the
/// write out just as well.
fn write_pulled(dir: &Path, path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        // Checked before anything is created: `create_dir_all` follows a
        // symlinked component, which would leave directories outside the paper
        // even though the refusal below stops the bytes.
        let mut existing = parent;
        while !existing.exists() {
            match existing.parent() {
                Some(up) => existing = up,
                None => break,
            }
        }
        if !std::fs::canonicalize(existing)?.starts_with(dir) {
            return Err(anyhow!(
                "{} resolves outside the paper's folder",
                parent.display()
            ));
        }
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(anyhow!("{} is a symlink", path.display()));
    }
    std::fs::write(path, bytes)?;
    Ok(())
}

fn hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

/// Overleaf compiles the project's *main document*, which it will not change
/// because we pushed a file. A paper landing beside a different root `.tex` is
/// therefore invisible until the user says which one is the main one — so this
/// keeps saying so, not only on the sync that first pushed it.
fn main_document_note(remote_files: &BTreeSet<String>, main: &str) -> Option<String> {
    let others: Vec<&str> = remote_files
        .iter()
        .filter(|rel| !rel.contains('/') && rel.to_ascii_lowercase().ends_with(".tex"))
        .map(String::as_str)
        .filter(|rel| *rel != main)
        .collect();
    (!others.is_empty()).then(|| {
        format!(
            "Overleaf's project also has {}. Set {main} as the main document there, or it will keep compiling the other file.",
            others.join(", ")
        )
    })
}

/// A page that posts the paper to Overleaf's project-creation endpoint as soon
/// as it loads. Every account can create a project this way, which is what
/// makes it the answer for one that cannot use the git bridge — files travel
/// inline as data URLs, so Overleaf never has to reach back to this machine.
pub fn upload_form_html(payload: &Payload) -> Result<String> {
    if payload.total_bytes() > MAX_UPLOAD_BYTES {
        return Err(anyhow!(
            "This paper and its figures are larger than {} MB, which is more than an Overleaf upload can carry. Link an Overleaf project and push over Git instead.",
            MAX_UPLOAD_BYTES / (1024 * 1024)
        ));
    }
    let engine = payload
        .files
        .get(&payload.main)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|source| crate::local::latex::program_from_source(&source))
        // Overleaf names its engines after the binaries, so the document's
        // `% !TeX program` line carries straight over.
        .map(|program| program.binary());
    let mut fields = String::new();
    for (rel, path) in &payload.files {
        let bytes = std::fs::read(path)?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let uri = format!("data:{};base64,{encoded}", mime_for(rel));
        fields.push_str(&hidden("snip_uri[]", &uri));
        fields.push_str(&hidden("snip_name[]", rel));
    }
    fields.push_str(&hidden("main_document", &payload.main));
    if let Some(engine) = engine {
        fields.push_str(&hidden("engine", engine));
    }
    Ok(format!(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>Opening Overleaf…</title></head>\
<body><p>Opening Overleaf…</p>\
<form id=\"upload\" method=\"post\" action=\"https://www.overleaf.com/docs\" accept-charset=\"utf-8\">{fields}\
<noscript><button type=\"submit\">Continue to Overleaf</button></noscript></form>\
<script>document.getElementById('upload').submit();</script></body></html>"
    ))
}

fn hidden(name: &str, value: &str) -> String {
    format!(
        "<input type=\"hidden\" name=\"{}\" value=\"{}\">",
        escape(name),
        escape(value)
    )
}

pub(crate) fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn mime_for(name: &str) -> &'static str {
    let extension = Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        "eps" => "application/postscript",
        _ => "text/plain",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(id: &str, host: &str) -> Project {
        Project {
            id: id.to_string(),
            host: host.to_string(),
        }
    }

    #[test]
    fn reads_a_project_out_of_every_form_a_user_can_paste() {
        let id = "64f0c1a2b3d4e5f60718293a";
        for input in [
            "https://www.overleaf.com/project/64f0c1a2b3d4e5f60718293a",
            "https://www.overleaf.com/project/64f0c1a2b3d4e5f60718293a/edit?foo=1",
            "https://git.overleaf.com/64f0c1a2b3d4e5f60718293a",
            "  64f0c1a2b3d4e5f60718293a  ",
        ] {
            let parsed = parse_project(input).expect(input);
            assert_eq!(parsed, project(id, "www.overleaf.com"), "{input}");
            assert_eq!(parsed.git_url(), format!("https://git.overleaf.com/{id}"));
        }
        // Server Pro serves the bridge from the site itself.
        let pro = parse_project("https://tex.lab.example/project/abc123").unwrap();
        assert_eq!(pro.git_url(), "https://tex.lab.example/git/abc123");
        assert_eq!(pro.web_url(), "https://tex.lab.example/project/abc123");

        for junk in ["", "not a url", "https://www.overleaf.com/", "ab"] {
            assert!(parse_project(junk).is_err(), "{junk:?} is not a project");
        }
    }

    #[test]
    fn collects_the_paper_with_what_it_references_and_nothing_else() {
        let temporary = TemporaryDirectory::new("orx-overleaf-test").unwrap();
        let root = temporary.path();
        let paper = root.join("paper");
        std::fs::create_dir_all(paper.join("figs")).unwrap();
        std::fs::write(paper.join("figs/loss.png"), b"png").unwrap();
        std::fs::write(paper.join("neurips.sty"), b"style").unwrap();
        std::fs::write(paper.join("refs.bib"), b"bib").unwrap();
        std::fs::write(root.join("secret.tex"), b"outside").unwrap();
        let tex = paper.join("paper.tex");
        std::fs::write(
            &tex,
            r"\includegraphics[width=0.8\linewidth]{figs/loss}
\bibliography{refs}
% \includegraphics{figs/unused.png}
\input{../secret}
\includegraphics{figs/missing.png}
",
        )
        .unwrap();

        let payload = collect(&tex).unwrap();
        let sent: Vec<&str> = payload.files.keys().map(String::as_str).collect();
        assert_eq!(
            sent,
            vec!["figs/loss.png", "neurips.sty", "paper.tex", "refs.bib"]
        );
        // `{figs/loss}` and `{refs}` both resolved once an extension was tried;
        // reporting the bare spelling as missing would be a false alarm on
        // every paper that writes them the normal way.
        assert!(
            !payload
                .skipped
                .iter()
                .any(|s| s == "figs/loss" || s == "refs"),
            "a reference that resolved is not missing: {:?}",
            payload.skipped
        );
        assert!(
            payload.skipped.iter().any(|s| s.contains("missing")),
            "a referenced file that is not there is reported: {:?}",
            payload.skipped
        );
        assert!(
            !payload.skipped.iter().any(|s| s.contains("unused")),
            "a commented-out figure is not a reference at all"
        );
    }

    /// A local bare repository standing in for an Overleaf project, seeded the
    /// way Overleaf seeds one: with a main file already in it.
    struct Fake {
        _temporary: TemporaryDirectory,
        root: PathBuf,
        remote: String,
        paper: PathBuf,
    }

    fn run(dir: &Path, args: &[&str]) -> String {
        let output = git(Some(dir), None, args).unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn commit_all(dir: &Path, message: &str) {
        run(dir, &["add", "-A"]);
        run(
            dir,
            &[
                "-c",
                "user.name=seed",
                "-c",
                "user.email=seed@example.com",
                "commit",
                "--quiet",
                "-m",
                message,
            ],
        );
    }

    impl Fake {
        fn new() -> Self {
            let temporary = TemporaryDirectory::new("orx-overleaf-test").unwrap();
            let root = temporary.path().to_path_buf();
            let bare = root.join("origin.git");
            std::fs::create_dir_all(&bare).unwrap();
            run(
                &bare,
                &["init", "--quiet", "--bare", "--initial-branch=master"],
            );

            let seed = root.join("seed");
            std::fs::create_dir_all(&seed).unwrap();
            run(&seed, &["init", "--quiet", "--initial-branch=master"]);
            std::fs::write(seed.join("main.tex"), b"\\documentclass{article}").unwrap();
            commit_all(&seed, "seed");
            run(&seed, &["remote", "add", "origin", &bare.to_string_lossy()]);
            run(&seed, &["push", "--quiet", "origin", "master"]);

            let paper = root.join("paper");
            std::fs::create_dir_all(paper.join("figs")).unwrap();
            std::fs::write(paper.join("figs/loss.png"), b"png").unwrap();
            std::fs::write(paper.join("paper.tex"), b"\\includegraphics{figs/loss.png}").unwrap();
            Fake {
                _temporary: temporary,
                remote: bare.to_string_lossy().to_string(),
                root,
                paper,
            }
        }

        fn sync(&self, baseline: &Baseline) -> SyncOutcome {
            self.sync_resolving(baseline, &BTreeMap::new())
        }

        fn sync_resolving(
            &self,
            baseline: &Baseline,
            resolutions: &BTreeMap<String, Resolution>,
        ) -> SyncOutcome {
            let payload = collect(&self.paper.join("paper.tex")).unwrap();
            sync_with(&payload, &self.remote, None, baseline, resolutions).unwrap()
        }

        /// Someone editing in Overleaf: change a file and push it.
        fn edit_on_overleaf(&self, rel: &str, contents: &[u8]) {
            let seed = self.root.join("seed");
            run(&seed, &["pull", "--quiet", "--rebase", "origin", "master"]);
            let path = seed.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
            commit_all(&seed, "edit on Overleaf");
            run(&seed, &["push", "--quiet", "origin", "master"]);
        }

        fn tree(&self) -> Vec<String> {
            let bare = self.root.join("origin.git");
            let mut names: Vec<String> = run(&bare, &["ls-tree", "-r", "--name-only", "master"])
                .lines()
                .map(str::to_string)
                .collect();
            names.sort();
            names
        }

        fn local(&self, rel: &str) -> String {
            std::fs::read_to_string(self.paper.join(rel)).unwrap()
        }
    }

    /// Drives the real clone-copy-commit-push sequence: everything a push does
    /// apart from Overleaf being on the other end.
    #[test]
    fn pushes_the_paper_then_reports_the_project_already_current() {
        let fake = Fake::new();

        let first = fake.sync(&Baseline::new());
        assert_eq!(first.pushed, vec!["figs/loss.png", "paper.tex"]);
        assert!(
            first
                .note
                .as_deref()
                .unwrap_or_default()
                .contains("main.tex"),
            "a project whose main document is another file must say so: {:?}",
            first.note
        );
        assert_eq!(fake.tree(), ["figs/loss.png", "main.tex", "paper.tex"]);

        let second = fake.sync(&first.baseline);
        assert!(
            second.pushed.is_empty() && second.pulled.is_empty(),
            "syncing the same files again must not make an empty commit"
        );
        assert_eq!(second.head, first.head, "and must not move Overleaf's HEAD");
    }

    /// The direction a change flows is decided by the baseline, not by which
    /// copy is newer on disk.
    #[test]
    fn carries_each_side_s_own_edit_the_other_way() {
        let fake = Fake::new();
        let first = fake.sync(&Baseline::new());

        // A co-author edits the paper in Overleaf, and adds a file we never had.
        fake.edit_on_overleaf("paper.tex", b"\\section{From Overleaf}");
        fake.edit_on_overleaf("refs.bib", b"@article{x}");

        let pulled = fake.sync(&first.baseline);
        assert_eq!(pulled.pulled, vec!["paper.tex", "refs.bib"]);
        assert!(pulled.conflicts.is_empty());
        assert_eq!(fake.local("paper.tex"), "\\section{From Overleaf}");
        assert_eq!(fake.local("refs.bib"), "@article{x}");

        // Now the agent rewrites it here, and that goes the other way.
        std::fs::write(fake.paper.join("paper.tex"), b"\\section{From orx}").unwrap();
        let pushed = fake.sync(&pulled.baseline);
        assert_eq!(pushed.pushed, vec!["paper.tex"]);
        assert!(pushed.pulled.is_empty() && pushed.conflicts.is_empty());
        assert_eq!(fake.local("paper.tex"), "\\section{From orx}");
    }

    /// The case that must never be resolved automatically: both sides wrote to
    /// the same file since they last agreed.
    #[test]
    fn leaves_a_file_both_sides_changed_alone_on_both_sides() {
        let fake = Fake::new();
        let first = fake.sync(&Baseline::new());

        fake.edit_on_overleaf("paper.tex", b"co-author's paragraph");
        std::fs::write(fake.paper.join("paper.tex"), b"the agent's paragraph").unwrap();

        let outcome = fake.sync(&first.baseline);
        assert_eq!(outcome.conflicts, vec!["paper.tex"]);
        assert!(outcome.pulled.is_empty() && outcome.pushed.is_empty());
        assert_eq!(
            fake.local("paper.tex"),
            "the agent's paragraph",
            "the local copy is untouched"
        );

        // The conflict must survive: resolving it silently on the next round is
        // the same data loss, one sync later.
        let again = fake.sync(&outcome.baseline);
        assert_eq!(again.conflicts, vec!["paper.tex"]);
        assert!(again.pushed.is_empty());
    }

    /// The sync and the dashboard name the same file differently, and a paper
    /// in a subdirectory is where that goes wrong.
    #[test]
    fn translates_between_the_paper_s_folder_and_the_checkout() {
        let nested = folder_of("papers/neurips/paper.tex");
        assert_eq!(nested.as_deref(), Some("papers/neurips"));
        assert_eq!(
            to_checkout(
                nested.as_deref(),
                &["paper.tex".into(), "figs/loss.png".into()]
            ),
            ["papers/neurips/paper.tex", "papers/neurips/figs/loss.png"]
        );
        assert_eq!(
            from_checkout(nested.as_deref(), "papers/neurips/figs/loss.png").as_deref(),
            Some("figs/loss.png")
        );
        // A path outside the paper's folder is not this paper's to settle.
        assert_eq!(from_checkout(nested.as_deref(), "other/paper.tex"), None);

        let root = folder_of("paper.tex");
        assert_eq!(root, None);
        assert_eq!(to_checkout(None, &["paper.tex".into()]), ["paper.tex"]);
        assert_eq!(
            from_checkout(None, "paper.tex").as_deref(),
            Some("paper.tex")
        );
        // At the root the two namespaces coincide, so a nested figure is still
        // this paper's to settle — dropping it would make the panel's conflict
        // buttons do nothing.
        assert_eq!(
            from_checkout(None, "figs/loss.png").as_deref(),
            Some("figs/loss.png")
        );
    }

    /// The mirror of the deletion test below: a co-author removing a figure in
    /// Overleaf must not have it pushed straight back.
    #[test]
    fn does_not_push_back_a_file_deleted_on_overleaf() {
        let fake = Fake::new();
        let first = fake.sync(&Baseline::new());
        assert!(first.pushed.contains(&"figs/loss.png".to_string()));

        let seed = fake.root.join("seed");
        run(&seed, &["pull", "--quiet", "--rebase", "origin", "master"]);
        std::fs::remove_file(seed.join("figs/loss.png")).unwrap();
        commit_all(&seed, "remove the figure on Overleaf");
        run(&seed, &["push", "--quiet", "origin", "master"]);

        let outcome = fake.sync(&first.baseline);
        assert!(
            !outcome.pushed.contains(&"figs/loss.png".to_string()),
            "a deletion made on Overleaf must not be undone: {:?}",
            outcome.pushed
        );
        assert!(!fake.tree().contains(&"figs/loss.png".to_string()));
        assert!(outcome
            .note
            .as_deref()
            .unwrap_or_default()
            .contains("Deleted on Overleaf"));

        // And it stays deleted. Holding the agreement is the whole point: drop
        // it and the sync after this one sees nothing to respect.
        let after = fake.sync(&outcome.baseline);
        assert!(
            !after.pushed.contains(&"figs/loss.png".to_string()),
            "the deletion must survive more than one sync: {:?}",
            after.pushed
        );
        assert!(!fake.tree().contains(&"figs/loss.png".to_string()));
    }

    /// A pull the budget refused must not become a push. Resolving it
    /// "use Overleaf's" and getting the local copy sent instead is the button
    /// doing the opposite of what it says.
    #[test]
    fn a_refused_pull_never_turns_into_a_push() {
        let fake = Fake::new();
        let first = fake.sync(&Baseline::new());

        // Fill the pull budget ahead of `paper.tex` in the remote tree's order.
        let seed = fake.root.join("seed");
        run(&seed, &["pull", "--quiet", "--rebase", "origin", "master"]);
        std::fs::create_dir_all(seed.join("figs")).unwrap();
        for i in 0..=MAX_FILES {
            std::fs::write(seed.join(format!("figs/f{i:03}.png")), format!("{i}")).unwrap();
        }
        std::fs::write(seed.join("paper.tex"), b"co-author's paper").unwrap();
        commit_all(&seed, "a big editing session");
        run(&seed, &["push", "--quiet", "origin", "master"]);
        std::fs::write(fake.paper.join("paper.tex"), b"our paper").unwrap();

        let outcome = fake.sync(&first.baseline);
        assert!(
            !outcome.pushed.contains(&"paper.tex".to_string()),
            "a refused pull must leave the file alone, not push over it: {:?}",
            outcome.pushed
        );

        // Even when the user explicitly asks for Overleaf's copy and the budget
        // still refuses it, the local copy must not go the other way.
        let settled = fake.sync_resolving(
            &outcome.baseline,
            &BTreeMap::from([("paper.tex".to_string(), Resolution::TakeOverleaf)]),
        );
        assert!(
            !settled.pushed.contains(&"paper.tex".to_string()),
            "\"use Overleaf's\" must never send the local copy: {:?}",
            settled.pushed
        );
    }

    /// A push that Overleaf rejects must leave the checkout exactly as it was:
    /// a half-applied sync is what silently reverts a co-author later.
    #[test]
    fn writes_nothing_locally_when_the_push_fails() {
        let fake = Fake::new();
        let first = fake.sync(&Baseline::new());
        // Overleaf moved the paper; we moved a figure. The sync therefore has
        // something to pull and something to push.
        fake.edit_on_overleaf("paper.tex", b"co-author's paragraph");
        std::fs::write(fake.paper.join("figs/loss.png"), b"regenerated").unwrap();
        // A remote that refuses the push, reached after the pull was planned.
        std::fs::write(
            fake.root.join("origin.git/hooks/pre-receive"),
            "#!/bin/sh\nexit 1\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                fake.root.join("origin.git/hooks/pre-receive"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }

        let payload = collect(&fake.paper.join("paper.tex")).unwrap();
        let failed = sync_with(
            &payload,
            &fake.remote,
            None,
            &first.baseline,
            &BTreeMap::new(),
        );
        assert!(failed.is_err(), "the push was rejected");
        assert_eq!(
            fake.local("paper.tex"),
            "\\includegraphics{figs/loss.png}",
            "nothing may be written locally until Overleaf has taken the push"
        );
    }

    /// The helper is a hand-written shell snippet, and it is the only thing
    /// standing between a project URL naming another host and the user's
    /// Overleaf token. Ask git itself what it would hand over.
    #[test]
    fn hands_the_token_only_to_the_host_the_project_was_linked_with() {
        let filled = |asked_for: &str| {
            let mut command = Command::new("git");
            command
                .env(HOST_ENV, "git.overleaf.com")
                .env(TOKEN_ENV, "olp_secret")
                .env("GIT_TERMINAL_PROMPT", "0")
                .args(["-c", "credential.helper="])
                .args(["-c", &format!("credential.helper={CREDENTIAL_HELPER}")])
                .args(["credential", "fill"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null());
            let mut child = command.spawn().unwrap();
            use std::io::Write;
            write!(
                child.stdin.as_mut().unwrap(),
                "protocol=https\nhost={asked_for}\n\n"
            )
            .unwrap();
            let output = child.wait_with_output().unwrap();
            String::from_utf8_lossy(&output.stdout).to_string()
        };

        assert!(
            filled("git.overleaf.com").contains("password=olp_secret"),
            "the linked host must be answered, or nothing can sync"
        );
        assert!(
            !filled("git.evil.example").contains("olp_secret"),
            "a project URL naming another host must not collect the token"
        );
    }

    /// The two guards that stand between a co-author's file name and the rest
    /// of the checkout.
    #[test]
    fn refuses_a_remote_path_that_leaves_the_paper_s_folder() {
        let dir = Path::new("/papers/neurips");
        assert_eq!(
            confined_path(dir, "figs/loss.png"),
            Some(dir.join("figs/loss.png"))
        );
        for hostile in ["../secret.tex", "/etc/passwd", ".github/workflows/ci.yml"] {
            assert_eq!(confined_path(dir, hostile), None, "{hostile}");
        }

        let temporary = TemporaryDirectory::new("orx-overleaf-test").unwrap();
        let paper = temporary.path().join("paper");
        let outside = temporary.path().join("outside");
        std::fs::create_dir_all(&paper).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, paper.join("figs")).unwrap();
            let through = paper.join("figs/loss.png");
            assert!(
                write_pulled(&paper, &through, b"png").is_err(),
                "a symlinked folder must not carry the write out of the paper"
            );
            assert!(!outside.join("loss.png").exists());
        }
    }

    /// The likeliest first run of all: linking a paper to the Overleaf project
    /// that already holds a copy of it. Neither side may be discarded.
    #[test]
    fn a_first_sync_onto_a_project_that_already_has_the_paper_conflicts() {
        let fake = Fake::new();
        fake.edit_on_overleaf("paper.tex", b"the version already on Overleaf");

        let outcome = fake.sync(&Baseline::new());
        assert_eq!(outcome.conflicts, vec!["paper.tex"]);
        // The figure is new to Overleaf, so it travels; only the file that
        // exists on both sides is ambiguous, and it moves in neither direction.
        assert_eq!(outcome.pushed, vec!["figs/loss.png"]);
        // And Overleaf's own `main.tex`, which this checkout never had, arrives
        // — a file only one side has is not ambiguous either.
        assert_eq!(outcome.pulled, vec!["main.tex"]);
        assert_eq!(fake.local("paper.tex"), "\\includegraphics{figs/loss.png}");
    }

    /// A file Overleaf has that the paper does not reference still has an
    /// agreement, and losing it would read a one-sided edit as a conflict.
    #[test]
    fn keeps_the_agreement_for_a_file_the_paper_does_not_reference() {
        let fake = Fake::new();
        let first = fake.sync(&Baseline::new());
        fake.edit_on_overleaf("notes.tex", b"co-author's notes");
        let pulled = fake.sync(&first.baseline);
        assert_eq!(pulled.pulled, vec!["notes.tex"]);

        // Edited here, untouched there. Nothing pushes it — it is not part of
        // the paper — but it must not become a conflict either.
        std::fs::write(fake.paper.join("notes.tex"), b"our notes").unwrap();
        let ours = fake.sync(&pulled.baseline);
        assert!(ours.conflicts.is_empty(), "{:?}", ours.conflicts);
        let again = fake.sync(&ours.baseline);
        assert!(again.conflicts.is_empty(), "{:?}", again.conflicts);
    }

    /// A conflict is the user's to settle, and settling it must actually clear
    /// it — in either direction.
    #[test]
    fn settles_a_conflict_the_way_the_user_asks() {
        for keep_local in [false, true] {
            let fake = Fake::new();
            let first = fake.sync(&Baseline::new());
            fake.edit_on_overleaf("paper.tex", b"co-author's paragraph");
            std::fs::write(fake.paper.join("paper.tex"), b"the agent's paragraph").unwrap();
            let conflicted = fake.sync(&first.baseline);
            assert_eq!(conflicted.conflicts, vec!["paper.tex"]);

            let resolution = if keep_local {
                Resolution::KeepLocal
            } else {
                Resolution::TakeOverleaf
            };
            let settled = fake.sync_resolving(
                &conflicted.baseline,
                &BTreeMap::from([("paper.tex".to_string(), resolution)]),
            );
            assert!(
                settled.conflicts.is_empty(),
                "{resolution:?} left a conflict"
            );
            let expected = if keep_local {
                "the agent's paragraph"
            } else {
                "co-author's paragraph"
            };
            assert_eq!(fake.local("paper.tex"), expected, "{resolution:?}");

            // And it stays settled: both sides now hold the chosen text.
            let after = fake.sync(&settled.baseline);
            assert!(after.conflicts.is_empty() && after.pulled.is_empty());
            assert_eq!(fake.local("paper.tex"), expected);
        }
    }

    /// A paper is usually a tree of sources, and a figure named by an
    /// `\input`ed section is as much part of it as one named by the entry file.
    #[test]
    fn follows_input_into_the_sections_it_pulls_in() {
        let fake = Fake::new();
        std::fs::create_dir_all(fake.paper.join("sections")).unwrap();
        std::fs::write(fake.paper.join("figs/curve.pdf"), b"pdf").unwrap();
        std::fs::write(
            fake.paper.join("sections/method.tex"),
            br"\includegraphics[
                width=\linewidth]{figs/curve}",
        )
        .unwrap();
        std::fs::write(fake.paper.join("paper.tex"), br"\input{sections/method}").unwrap();

        let payload = collect(&fake.paper.join("paper.tex")).unwrap();
        let sent: Vec<&str> = payload.files.keys().map(String::as_str).collect();
        assert!(
            sent.contains(&"sections/method.tex") && sent.contains(&"figs/curve.pdf"),
            "a figure named by an \\input-ed section travels too: {sent:?}"
        );
        assert!(payload.skipped.is_empty(), "{:?}", payload.skipped);
    }

    /// The Overleaf project is a co-author's to edit, and this syncs into a
    /// checkout that orx runs experiments out of.
    #[test]
    fn refuses_to_pull_a_file_that_is_not_part_of_a_paper() {
        let fake = Fake::new();
        let first = fake.sync(&Baseline::new());
        fake.edit_on_overleaf("Makefile", b"all:\n\trm -rf /");
        fake.edit_on_overleaf(".github/workflows/ci.yml", b"on: push");

        let outcome = fake.sync(&first.baseline);
        assert!(outcome.pulled.is_empty(), "{:?}", outcome.pulled);
        assert!(!fake.paper.join("Makefile").exists());
        assert!(!fake.paper.join(".github").exists());
        assert!(outcome
            .note
            .as_deref()
            .unwrap_or_default()
            .contains("Left on Overleaf"));
    }

    /// Deleting a figure here must not be undone by the next pull.
    #[test]
    fn does_not_pull_back_a_file_deleted_here() {
        let fake = Fake::new();
        let first = fake.sync(&Baseline::new());
        std::fs::remove_file(fake.paper.join("figs/loss.png")).unwrap();

        let outcome = fake.sync(&first.baseline);
        assert!(outcome.pulled.is_empty(), "{:?}", outcome.pulled);
        assert!(!fake.paper.join("figs/loss.png").exists());
        assert!(outcome
            .note
            .as_deref()
            .unwrap_or_default()
            .contains("deleted here"));
    }

    #[test]
    fn separates_a_plan_that_cannot_push_from_a_token_that_cannot() {
        assert_eq!(
            classify("fatal: unable to access 'https://git.overleaf.com/x/': The requested URL returned error: 403"),
            Some(Refusal::Unsupported)
        );
        assert_eq!(
            classify("remote: Invalid authentication credentials\nfatal: Authentication failed for 'https://git.overleaf.com/x/'"),
            Some(Refusal::Auth)
        );
        assert_eq!(
            classify("fatal: repository 'https://git.overleaf.com/x/' not found"),
            Some(Refusal::NotFound)
        );
        // A project id is 24 hex characters and can contain "403" by chance;
        // the code must come from git's own wording, not from the URL.
        assert_eq!(
            classify("fatal: unable to access 'https://git.overleaf.com/64f403a1b2c3d4e5f6071829/': Could not resolve host"),
            None
        );
        assert_eq!(classify("fatal: could not read from remote"), None);
    }

    #[test]
    fn the_upload_page_posts_every_file_as_a_data_url() {
        let temporary = TemporaryDirectory::new("orx-overleaf-test").unwrap();
        let tex = temporary.path().join("pa\"per.tex");
        std::fs::write(&tex, b"% !TeX program = lualatex\nhi").unwrap();
        let payload = collect(&tex).unwrap();

        let html = upload_form_html(&payload).unwrap();
        assert!(html.contains("action=\"https://www.overleaf.com/docs\""));
        assert!(html.contains("data:text/plain;base64,"));
        assert!(html.contains("name=\"engine\" value=\"lualatex\""));
        // A quote in a file name must not break out of the attribute.
        assert!(html.contains("pa&quot;per.tex"));
        assert!(!html.contains("value=\"pa\"per.tex\""));
    }
}
