use anyhow::{anyhow, bail, Result};
use std::future::Future;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;
use tokio::task::JoinHandle;

pub type Prompt = Arc<
    dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<Option<String>>> + Send>> + Send + Sync,
>;

pub(super) struct Askpass {
    endpoint: String,
    token: String,
    task: JoinHandle<()>,
    auth_prompted: Arc<AtomicBool>,
}

impl Askpass {
    pub async fn start(prompt: Prompt) -> Result<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let endpoint = listener.local_addr()?.to_string();
        let token = uuid::Uuid::new_v4().to_string();
        let expected = token.clone();
        let auth_prompted = Arc::new(AtomicBool::new(false));
        let invoked = auth_prompted.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let mut stream = BufReader::new(stream);
                let exchange = async {
                    let mut line = String::new();
                    (&mut stream).take(16384).read_line(&mut line).await?;
                    let (token, question): (String, String) = serde_json::from_str(&line)?;
                    if token != expected {
                        bail!("Invalid askpass token");
                    }
                    let host_key = question.contains("continue connecting (yes/no")
                        || question.starts_with("Please type 'yes' or 'no':")
                        || question.starts_with("Please type 'yes', 'no' or the fingerprint:")
                        || question.starts_with("Accept updated hostkeys? (yes/no):");
                    if !host_key {
                        invoked.store(true, Ordering::SeqCst);
                    }
                    let answer = prompt(question).await?;
                    let bytes = serde_json::to_vec(&answer)?;
                    stream.get_mut().write_all(&bytes).await?;
                    stream.get_mut().write_all(b"\n").await?;
                    anyhow::Ok(())
                };
                let _ = tokio::time::timeout(std::time::Duration::from_secs(300), exchange).await;
            }
        });
        Ok(Self {
            endpoint,
            token,
            task,
            auth_prompted,
        })
    }

    pub fn auth_prompted(&self) -> bool {
        self.auth_prompted.load(Ordering::SeqCst)
    }

    pub fn configure(&self, command: &mut Command, executable: &std::path::Path) {
        command
            .env("SSH_ASKPASS", executable)
            .env("SSH_ASKPASS_REQUIRE", "force")
            .env("DISPLAY", "orx-engine")
            .env("ORX_ENGINE_ASKPASS", &self.endpoint)
            .env("ORX_ENGINE_ASKPASS_TOKEN", &self.token)
            .stdin(std::process::Stdio::null());
        // Askpass is a console program: a detached ssh would open a window for it. Batch
        // logins stay detached, so a ProxyJump hop fails instead of waiting on a hidden console.
        #[cfg(windows)]
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
}

impl Drop for Askpass {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub async fn relay() -> Result<()> {
    let stream = TcpStream::connect(std::env::var("ORX_ENGINE_ASKPASS")?).await?;
    let mut stream = BufReader::new(stream);
    let token = std::env::var("ORX_ENGINE_ASKPASS_TOKEN")?;
    let question = std::env::args().nth(1).unwrap_or_default();
    let bytes = serde_json::to_vec(&(token, question))?;
    stream.get_mut().write_all(&bytes).await?;
    stream.get_mut().write_all(b"\n").await?;
    let mut line = String::new();
    (&mut stream).take(16384).read_line(&mut line).await?;
    let answer: Option<String> = serde_json::from_str(&line)?;
    let answer = answer.ok_or_else(|| anyhow!("SSH authentication cancelled"))?;
    if answer.contains(['\n', '\r']) {
        bail!("Invalid multiline SSH answer");
    }
    println!("{answer}");
    Ok(())
}

/// A host's saved password and passphrase answers. A password answers its prompt
/// once per login attempt and a passphrase twice, since a ProxyJump hop decrypts
/// the same key again; asked once more, it was rejected and the user decides.
pub(crate) struct Saved {
    alias: String,
    state: std::sync::Mutex<SavedState>,
}

#[derive(Default)]
struct SavedState {
    /// Read from the keychain at the first secret prompt.
    loaded: Option<crate::local::keychain::Answers>,
    used: std::collections::BTreeMap<String, usize>,
    rejected: std::collections::BTreeSet<String>,
    typed: crate::local::keychain::Answers,
}

impl Saved {
    pub(crate) fn new(alias: &str) -> Arc<Self> {
        Arc::new(Self {
            alias: alias.to_owned(),
            state: Default::default(),
        })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, SavedState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A keychain that can't be read answers nothing, so the user is asked. The
    /// keychain may wait on an unlock dialog, so it is read off the async workers.
    async fn loaded(&self) -> crate::local::keychain::Answers {
        if let Some(answers) = &self.state().loaded {
            return answers.clone();
        }
        let alias = self.alias.clone();
        let answers = tokio::task::spawn_blocking(move || crate::local::keychain::load(&alias))
            .await
            .map_err(|error| anyhow!("{error}"))
            .and_then(|answers| answers)
            .unwrap_or_else(|error| {
                eprintln!("Host {}: {error:#}", self.alias);
                Default::default()
            });
        self.state().loaded.get_or_insert(answers).clone()
    }

    pub(crate) async fn has_answers(&self) -> bool {
        !self.loaded().await.is_empty()
    }

    /// Answers secret prompts it has saved, then defers to `user`; without one,
    /// any other prompt is declined.
    pub(crate) fn wrap(self: &Arc<Self>, user: Option<Prompt>) -> Prompt {
        let saved = self.clone();
        Arc::new(move |question| {
            let saved = saved.clone();
            let user = user.clone();
            Box::pin(async move {
                let key = secret_key(&question);
                if let Some(key) = &key {
                    let answers = saved.loaded().await;
                    let mut state = saved.state();
                    let uses = if key.starts_with("passphrase ") { 2 } else { 1 };
                    let used = state.used.get(key).copied().unwrap_or(0);
                    if used >= uses {
                        state.rejected.insert(key.clone());
                    } else if let Some(answer) = answers.get(key) {
                        state.used.insert(key.clone(), used + 1);
                        return Ok(Some(answer.clone()));
                    }
                }
                let Some(user) = user else {
                    return Ok(None);
                };
                let answer = user(question).await?;
                if let (Some(key), Some(answer)) = (key, &answer) {
                    saved.state().typed.insert(key, answer.clone());
                }
                Ok(answer)
            })
        })
    }

    /// After the attempt: a login keeps what the user typed, and a rejected
    /// saved answer is dropped either way.
    pub(crate) async fn finish(&self, authenticated: bool) {
        let (rejected, typed) = {
            let state = self.state();
            if state.rejected.is_empty() && (state.typed.is_empty() || !authenticated) {
                return;
            }
            (state.rejected.clone(), state.typed.clone())
        };
        let alias = self.alias.clone();
        let result = tokio::task::spawn_blocking(move || {
            let mut answers = crate::local::keychain::load(&alias)?;
            answers.retain(|key, _| !rejected.contains(key));
            if authenticated {
                answers.extend(typed);
            }
            crate::local::keychain::save(&alias, &answers)
        })
        .await
        .map_err(|error| anyhow!("{error}"))
        .and_then(|result| result);
        if let Err(error) = result {
            eprintln!("Host {}: {error:#}", self.alias);
        }
    }
}

/// The prompts worth saving: passwords keyed by their text, key passphrases by
/// key path so ssh's retry prompt counts as the same one. Never one-time codes,
/// host keys or a password change.
fn secret_key(question: &str) -> Option<String> {
    let question = question.trim();
    let lower = question.to_ascii_lowercase();
    if let Some(at) = lower.find("passphrase") {
        let after = question[at..].split_once(" for ")?.1;
        let path = after
            .trim()
            .trim_end_matches(':')
            .trim()
            .trim_start_matches("key ")
            .trim_matches(['\'', '"']);
        return (!path.is_empty()).then(|| format!("passphrase {path}"));
    }
    let one_time = lower.contains("one-time")
        || lower
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| word == "otp");
    (lower.ends_with("password:") && !lower.contains("new password") && !one_time)
        .then(|| question.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(answers: &'static [&'static str]) -> (Prompt, Arc<std::sync::Mutex<Vec<String>>>) {
        let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = asked.clone();
        let prompt: Prompt = Arc::new(move |question| {
            let mut asked = log.lock().unwrap();
            asked.push(question);
            let answer = answers
                .get(asked.len() - 1)
                .map(|answer| answer.to_string());
            Box::pin(async move { Ok(answer) })
        });
        (prompt, asked)
    }

    #[test]
    fn only_passwords_and_passphrases_are_saved() {
        for (question, key) in [
            ("clo@gpu1's password: ", Some("clo@gpu1's password:")),
            ("(clo@gpu1) Password:", Some("(clo@gpu1) Password:")),
            (
                "Enter passphrase for key '/home/me/.ssh/id_ed25519': ",
                Some("passphrase /home/me/.ssh/id_ed25519"),
            ),
            (
                "Bad passphrase, try again for /home/me/.ssh/id_ed25519: ",
                Some("passphrase /home/me/.ssh/id_ed25519"),
            ),
            ("Verification code: ", None),
            (
                "Are you sure you want to continue connecting (yes/no/[fingerprint])? ",
                None,
            ),
            ("New password: ", None),
            ("One-time password: ", None),
            ("(clo@gpu1) OTP password:", None),
        ] {
            assert_eq!(secret_key(question).as_deref(), key, "{question}");
        }
    }

    fn with_keychain() -> bool {
        if !crate::config::in_test_child(&[]) {
            return false;
        }
        std::env::set_var(crate::local::keychain::TEST_KEYCHAIN, "1");
        true
    }

    #[tokio::test]
    async fn a_saved_answer_is_tried_once_then_the_user_decides_and_is_remembered() {
        if !with_keychain() {
            return;
        }
        let host = "keychain-retry";
        let password = "clo@gpu1's password: ".to_string();
        crate::local::keychain::save(
            host,
            &[(password.trim().to_owned(), "old".to_owned())].into(),
        )
        .unwrap();

        let saved = Saved::new(host);
        let (prompt, asked) = user(&["new", "123456"]);
        let prompt = saved.wrap(Some(prompt));
        assert_eq!(
            prompt(password.clone()).await.unwrap().as_deref(),
            Some("old")
        );
        assert_eq!(
            prompt(password.clone()).await.unwrap().as_deref(),
            Some("new")
        );
        assert_eq!(
            prompt("Verification code: ".into())
                .await
                .unwrap()
                .as_deref(),
            Some("123456")
        );
        assert_eq!(asked.lock().unwrap().len(), 2);
        saved.finish(true).await;
        assert_eq!(
            crate::local::keychain::load(host).unwrap(),
            [(password.trim().to_owned(), "new".to_owned())].into()
        );
    }

    #[tokio::test]
    async fn without_a_user_a_rejected_answer_is_forgotten_and_nothing_else_answers() {
        if !with_keychain() {
            return;
        }
        let host = "keychain-background";
        let password = "Password:".to_string();
        crate::local::keychain::save(host, &[(password.clone(), "stale".to_owned())].into())
            .unwrap();
        let saved = Saved::new(host);
        assert!(saved.has_answers().await);
        let prompt = saved.wrap(None);
        assert_eq!(
            prompt(password.clone()).await.unwrap().as_deref(),
            Some("stale")
        );
        assert_eq!(prompt(password.clone()).await.unwrap(), None);
        assert_eq!(prompt("Verification code:".into()).await.unwrap(), None);
        saved.finish(false).await;
        assert!(crate::local::keychain::load(host).unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_saved_passphrase_answers_each_proxyjump_hop() {
        if !with_keychain() {
            return;
        }
        let host = "keychain-hops";
        let question = "Enter passphrase for key '/home/me/.ssh/id_ed25519': ".to_string();
        crate::local::keychain::save(
            host,
            &[(
                "passphrase /home/me/.ssh/id_ed25519".to_owned(),
                "secret".to_owned(),
            )]
            .into(),
        )
        .unwrap();
        let saved = Saved::new(host);
        let (prompt, asked) = user(&["typed"]);
        let prompt = saved.wrap(Some(prompt));
        for _ in 0..2 {
            assert_eq!(
                prompt(question.clone()).await.unwrap().as_deref(),
                Some("secret")
            );
        }
        assert_eq!(
            prompt(question.clone()).await.unwrap().as_deref(),
            Some("typed")
        );
        assert_eq!(asked.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_failed_login_saves_nothing_it_was_told() {
        if !with_keychain() {
            return;
        }
        let host = "keychain-failed";
        let saved = Saved::new(host);
        let (prompt, _) = user(&["typo"]);
        let prompt = saved.wrap(Some(prompt));
        prompt("Password:".into()).await.unwrap();
        saved.finish(false).await;
        assert!(crate::local::keychain::load(host).unwrap().is_empty());
    }
}
