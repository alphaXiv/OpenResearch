//! Starter prompts for a project's empty chat: a brief of what the project
//! actually contains — paper, README, code, manifests — handed to a headless
//! harness child that writes four prompts about *this* project. Results are
//! cached per brief fingerprint so reopening the empty state doesn't pay for
//! another model call.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::harness::{OneShot, OneShotQuality};
use super::model::LocalProject;

const PAPER_PDF: &str = "paper.pdf";
const MAX_LISTED_FILES: usize = 5000;
const MAX_WALK_DEPTH: usize = 4;
/// Paths shown to the model; shallow ones first so the layout reads at a glance.
const MAX_TREE_PATHS: usize = 120;
const MAX_ENTRYPOINTS: usize = 3;
const MAX_MANIFESTS: usize = 3;
const README_CHARS: usize = 4000;
const PAPER_CHARS: usize = 6000;
const ENTRYPOINT_CHARS: usize = 2500;
const MANIFEST_CHARS: usize = 1200;
const PAPER_FETCH_TIMEOUT: Duration = Duration::from_secs(12);
/// A cold CLI start plus one reasoning-model round trip over a long brief.
const GENERATION_TIMEOUT: Duration = Duration::from_secs(90);
/// A failed generation (harness missing, not signed in, garbage reply) is
/// remembered this long so re-entering the empty state fails fast.
const FAILURE_TTL: Duration = Duration::from_secs(60);
/// Model children running at once, across on-demand, warm, and pre-warm.
const MAX_CONCURRENT: usize = 2;
const STARTER_TITLE_CHARS: usize = 60;
const STARTER_PROMPT_CHARS: usize = 600;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StarterPrompt {
    pub title: String,
    pub prompt: String,
}

const SETUP_FILES: &[&str] = &[
    "requirements.txt",
    "pyproject.toml",
    "environment.yml",
    "environment.yaml",
    "setup.py",
    "Makefile",
    "package.json",
    "Cargo.toml",
];

/// Launch-script name prefixes, strongest signal first.
const ENTRYPOINT_PREFIXES: &[&str] = &[
    "train",
    "main",
    "pretrain",
    "finetune",
    "fine_tune",
    "run",
    "experiment",
    "evaluate",
    "eval",
];

/// Four prompts about this project, from the cache or a fresh model call.
/// `None` when the harness can't answer (not installed, timed out, replied
/// with something that isn't four prompts).
pub async fn prompts(
    project: &LocalProject,
    harness_id: &str,
    locale: &str,
) -> Option<Vec<StarterPrompt>> {
    let brief = tokio::task::spawn_blocking({
        let project = project.clone();
        move || brief(&project)
    })
    .await
    .ok()?;
    generate(brief, project.paper_id.as_deref(), harness_id, locale).await
}

/// Generate for a project that does not exist yet, from what the new-project
/// form already knows: the name, and a paper id or an existing repository.
/// The brief is built exactly as it will be once the project is created, so
/// the cache entry is found by content the moment the empty chat opens.
pub fn prewarm(name: String, paper_id: Option<String>, path: Option<String>, locale: String) {
    tokio::spawn(async move {
        let harness = resolve_harness().await?;
        let brief = tokio::task::spawn_blocking({
            let paper_id = paper_id.clone();
            move || match path {
                Some(path) => {
                    // Creation registers the enclosing repository, not the
                    // typed folder, so brief the same root.
                    let repo = super::git::repository_root(Path::new(&path)).ok()?;
                    let files = list_files(&repo);
                    Some(brief_parts(&name, None, paper_id.as_deref(), &repo, &files))
                }
                None => {
                    let files = Vec::from_iter(paper_id.is_some().then(|| PAPER_PDF.to_string()));
                    Some(brief_parts(
                        &name,
                        None,
                        paper_id.as_deref(),
                        Path::new(""),
                        &files,
                    ))
                }
            }
        })
        .await
        .ok()??;
        generate(brief, paper_id.as_deref(), &harness, &locale).await
    });
}

/// Cache by content (brief, harness, locale) so a pre-warmed entry serves the
/// project created after it. The paper text (a network fetch) is fixed for a
/// paper id, so it is fetched only on a miss and never decides staleness.
async fn generate(
    brief: String,
    paper_id: Option<&str>,
    harness_id: &str,
    locale: &str,
) -> Option<Vec<StarterPrompt>> {
    let key = fingerprint(&brief, harness_id, locale);
    // Serialize per brief: an in-flight pre-warm, the dev double-effect, and a
    // reopened empty state all wait on the first call's cache entry.
    let lock = brief_lock(&key);
    let _guard = lock.lock().await;
    match lock_map(cache()).get(&key) {
        Some(Cached::Prompts(prompts)) => return Some(prompts.clone()),
        Some(Cached::Failed(at)) if at.elapsed() < FAILURE_TTL => return None,
        _ => {}
    }
    let prompts = generate_uncached(brief, paper_id, harness_id, locale).await;
    let mut cache = lock_map(cache());
    if cache.len() >= MAX_CACHED {
        cache.clear();
    }
    let entry = match &prompts {
        Some(prompts) => Cached::Prompts(prompts.clone()),
        None => Cached::Failed(Instant::now()),
    };
    cache.insert(key, entry);
    prompts
}

async fn generate_uncached(
    mut brief: String,
    paper_id: Option<&str>,
    harness_id: &str,
    locale: &str,
) -> Option<Vec<StarterPrompt>> {
    let harness = super::harness::chat_harness(harness_id)?;
    if let Some(paper_id) = paper_id {
        if let Some(text) = paper_text(paper_id).await {
            push(&mut brief, "Paper overview", &head(&text, PAPER_CHARS));
        }
    }
    let prompt = generation_prompt(&brief, locale);
    static SLOTS: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
    let _slot = SLOTS
        .get_or_init(|| tokio::sync::Semaphore::new(MAX_CONCURRENT))
        .acquire()
        .await
        .ok()?;
    let raw = harness
        .one_shot(OneShot {
            system: SYSTEM_PROMPT,
            prompt: &prompt,
            quality: OneShotQuality::Standard,
            timeout: GENERATION_TIMEOUT,
        })
        .await?;
    parse_prompts(&raw)
}

/// The user's preferred chat harness, else the first one that is ready.
async fn resolve_harness() -> Option<String> {
    let preferred = tokio::task::spawn_blocking(|| {
        crate::store::Store::open()
            .and_then(|store| store.ui_state())
            .ok()
            .and_then(|state| state.preferred_agent)
            .map(|agent| agent.harness)
    })
    .await
    .ok()
    .flatten()
    .filter(|harness| super::harness::is_chat_harness(harness));
    match preferred {
        Some(harness) => Some(harness),
        None => super::harness::detect_harnesses()
            .await
            .into_iter()
            .find(|info| info.agent_ready)
            .map(|info| info.id.to_string()),
    }
}

/// Generate in the background for a just-created project, so the first open
/// of its empty chat hits the cache. Fire-and-forget: any failure just leaves
/// the on-demand path to run.
pub fn warm(project: LocalProject, locale: String) {
    tokio::spawn(async move {
        let harness = resolve_harness().await?;
        prompts(&project, &harness, &locale).await
    });
}

/// Fingerprint → outcome. Small and rebuilt on restart; cleared wholesale
/// rather than evicted, since a hit is only ever the current brief anyway.
type Cache = Mutex<HashMap<String, Cached>>;
const MAX_CACHED: usize = 64;

enum Cached {
    Prompts(Vec<StarterPrompt>),
    Failed(Instant),
}

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// These maps hold plain data with no invariant a panic could break.
fn lock_map<T>(map: &Mutex<T>) -> MutexGuard<'_, T> {
    map.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn brief_lock(key: &str) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    let mut locks = lock_map(LOCKS.get_or_init(Default::default));
    if locks.len() >= MAX_CACHED {
        // Only an in-flight caller holds a second reference to its entry.
        locks.retain(|_, lock| Arc::strong_count(lock) > 1);
    }
    locks.entry(key.to_string()).or_default().clone()
}

fn fingerprint(brief: &str, harness_id: &str, locale: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(harness_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(locale.as_bytes());
    hasher.update(b"\0");
    hasher.update(brief.as_bytes());
    format!("{:x}", hasher.finalize())
}

const SYSTEM_PROMPT: &str = "You help a researcher start work in an AI research \
workspace where a coding agent runs experiments for them. You reply with JSON only.";

fn generation_prompt(brief: &str, locale: &str) -> String {
    format!(
        "Below is a brief of a research project a user just opened. Write the four \
         messages the user should send to their research agent, in order, to move \
         from this exact starting point to running a first experiment.\n\n\
         Rules:\n\
         - Every prompt must be about THIS project: name its actual method, files, \
         datasets, models, hyperparameters, claims, or gaps from the brief. Generic \
         advice that would fit any project is wrong.\n\
         - The four prompts progress: (1) understand the specific starting point, \
         (2) find the open question or weakness worth testing, (3) get a runnable \
         baseline with a concrete run command, (4) launch a first experiment with \
         one concrete, cheap-to-test hypothesis.\n\
         - Write each prompt as the user speaking to the agent, 1-3 sentences, \
         plain text, no markdown.\n\
         - Each title is at most five words.\n\
         - Write titles and prompts in the language with IETF tag \"{locale}\".\n\
         - Reply with a JSON array of exactly four objects with keys \"title\" and \
         \"prompt\", and nothing else.\n\n\
         Project brief:\n{brief}"
    )
}

/// Model output → four prompts. Tolerates code fences and prose around the
/// array; rejects anything that isn't four usable entries.
fn parse_prompts(raw: &str) -> Option<Vec<StarterPrompt>> {
    let start = raw.find('[')?;
    let end = raw.rfind(']')?;
    if end <= start {
        return None;
    }
    let parsed: Vec<StarterPrompt> = serde_json::from_str(&raw[start..=end]).ok()?;
    let prompts: Vec<StarterPrompt> = parsed
        .into_iter()
        .map(|p| StarterPrompt {
            title: clip(
                &p.title.split_whitespace().collect::<Vec<_>>().join(" "),
                STARTER_TITLE_CHARS,
            ),
            prompt: clip(p.prompt.trim(), STARTER_PROMPT_CHARS),
        })
        .filter(|p| !p.title.is_empty() && !p.prompt.is_empty())
        .take(4)
        .collect();
    (prompts.len() == 4).then_some(prompts)
}

fn clip(text: &str, chars: usize) -> String {
    text.chars()
        .take(chars)
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// Everything local the model gets to read about the project, in a stable
/// order (the fingerprint depends on it).
fn brief(project: &LocalProject) -> String {
    let repo = Path::new(&project.repo_path);
    let files = list_files(repo);
    brief_parts(
        &project.name,
        project.run_command.as_deref(),
        project.paper_id.as_deref(),
        repo,
        &files,
    )
}

fn brief_parts(
    name: &str,
    run_command: Option<&str>,
    paper_id: Option<&str>,
    repo: &Path,
    files: &[String],
) -> String {
    let mut out = String::new();
    push(&mut out, "Project name", name);
    if let Some(command) = run_command.filter(|c| !c.trim().is_empty()) {
        push(&mut out, "Run command", command);
    }
    if let Some(paper_id) = paper_id {
        push(&mut out, "Starting paper (arXiv id)", paper_id);
    }
    if files.iter().any(|f| f == PAPER_PDF) {
        push(
            &mut out,
            "Note",
            "paper.pdf is checked into the project root.",
        );
    }
    if let Some(readme) = files
        .iter()
        .find(|f| f.to_ascii_lowercase().starts_with("readme"))
    {
        if let Some(text) = read_head(&repo.join(readme)) {
            push(&mut out, readme, &head(&text, README_CHARS));
        }
    }
    let manifests: Vec<&str> = SETUP_FILES
        .iter()
        .copied()
        .filter(|name| files.iter().any(|f| f == name))
        .take(MAX_MANIFESTS)
        .collect();
    for manifest in manifests {
        if let Some(text) = read_head(&repo.join(manifest)) {
            push(&mut out, manifest, &head(&text, MANIFEST_CHARS));
        }
    }
    for entrypoint in entrypoints(files) {
        if let Some(text) = read_head(&repo.join(&entrypoint)) {
            push(&mut out, &entrypoint, &head(&text, ENTRYPOINT_CHARS));
        }
    }
    if files.is_empty() {
        push(&mut out, "Files", "(the project folder is empty)");
    } else {
        let mut tree: Vec<&String> = files.iter().collect();
        tree.sort_by_key(|f| (f.matches('/').count(), f.as_str()));
        let listing = tree
            .iter()
            .take(MAX_TREE_PATHS)
            .map(|f| f.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let label = if files.len() > MAX_TREE_PATHS {
            format!("Files ({} of {} shown)", MAX_TREE_PATHS, files.len())
        } else {
            format!("Files ({})", files.len())
        };
        push(&mut out, &label, &listing);
    }
    out
}

fn push(out: &mut String, label: &str, body: &str) {
    out.push_str("## ");
    out.push_str(label);
    out.push('\n');
    out.push_str(body.trim_end());
    out.push_str("\n\n");
}

fn head(text: &str, chars: usize) -> String {
    let mut out: String = text.chars().take(chars).collect();
    if text.chars().nth(chars).is_some() {
        out.push_str("\n[…truncated]");
    }
    out
}

/// alphaXiv's generated overview of the paper, else the opening of its full
/// text. Best-effort and bounded: the brief stands without it. Remembered per
/// paper id — the text is static and the fetch is the slow part of a miss.
async fn paper_text(paper_id: &str) -> Option<String> {
    static TEXTS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    let texts = TEXTS.get_or_init(Default::default);
    if let Some(text) = lock_map(texts).get(paper_id) {
        return Some(text.clone());
    }
    let fetch = async {
        for kind in ["overview", "abs"] {
            if let Ok(Some(text)) = crate::client::fetch_paper_markdown(kind, paper_id).await {
                if !text.trim().is_empty() {
                    return Some(text);
                }
            }
        }
        None
    };
    let text = tokio::time::timeout(PAPER_FETCH_TIMEOUT, fetch)
        .await
        .ok()
        .flatten()?;
    lock_map(texts).insert(paper_id.to_string(), text.clone());
    Some(text)
}

/// Tracked and untracked files honoring gitignore, or a shallow walk for a
/// folder git cannot list.
fn list_files(repo: &Path) -> Vec<String> {
    let mut files = match super::git::list_worktree_files(repo) {
        Ok(files) if !files.is_empty() => files,
        _ => walk(repo),
    };
    files.sort();
    files.dedup();
    // Keep the shallow paths when capping: README, manifests, and launch
    // scripts live near the root, and deep trees sort ahead of them by name.
    files.sort_by_key(|f| f.matches('/').count());
    files.truncate(MAX_LISTED_FILES);
    files
}

fn walk(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut pending = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if out.len() >= MAX_LISTED_FILES {
                return out;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name == "node_modules" || name == "__pycache__" {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                if depth + 1 < MAX_WALK_DEPTH {
                    pending.push((path, depth + 1));
                }
            } else if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out
}

/// Shallowest first, then by how strongly the name suggests a launch script
/// (`train.py` over `eval_*.py`), so the first entry is the natural start.
fn entrypoints(files: &[String]) -> Vec<String> {
    let mut found: Vec<(usize, usize, &String)> = files
        .iter()
        .filter_map(|file| {
            let name = file.rsplit('/').next().unwrap_or(file);
            let stem = name.strip_suffix(".py")?;
            let rank = ENTRYPOINT_PREFIXES.iter().position(|prefix| {
                stem == *prefix
                    || stem
                        .strip_prefix(prefix)
                        .is_some_and(|rest| rest.starts_with(['_', '-']))
            })?;
            let depth = file.matches('/').count();
            (depth <= 2).then_some((depth, rank, file))
        })
        .collect();
    found.sort();
    found
        .into_iter()
        .take(MAX_ENTRYPOINTS)
        .map(|(_, _, file)| file.clone())
        .collect()
}

fn read_head(path: &Path) -> Option<String> {
    use std::io::Read as _;
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(64 * 1024).read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn project(root: &Path) -> LocalProject {
        LocalProject {
            id: "p1".into(),
            name: "demo".into(),
            slug: "demo".into(),
            github_owner: String::new(),
            github_repo: String::new(),
            github_sync_enabled: false,
            baseline_branch: "main".into(),
            repo_path: root.to_string_lossy().into_owned(),
            run_command: Some("python train.py --steps 10".into()),
            paper_id: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn temp_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!("orx-starter-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn brief_reads_readme_manifests_entrypoints_and_tree() {
        let root = temp_root();
        write(&root, "README.md", "# Tiny Transformer\n\nA small model.\n");
        write(&root, "requirements.txt", "torch>=2.0\n");
        write(&root, "train.py", "import torch\nprint('hi')\n");
        write(&root, "src/model.py", "");
        let brief = brief(&project(&root));

        assert!(brief.contains("## Project name\ndemo"));
        assert!(brief.contains("## Run command\npython train.py --steps 10"));
        assert!(brief.contains("## README.md\n# Tiny Transformer"));
        assert!(brief.contains("## requirements.txt\ntorch>=2.0"));
        assert!(brief.contains("## train.py\nimport torch"));
        assert!(brief.contains("## Files (4)\nREADME.md\nrequirements.txt\ntrain.py\nsrc/model.py"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn brief_of_an_empty_folder_says_so() {
        let root = temp_root();
        let mut project = project(&root);
        project.run_command = None;
        let brief = brief(&project);
        assert!(brief.contains("(the project folder is empty)"));
        assert!(!brief.contains("Run command"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn head_marks_truncation() {
        assert_eq!(head("abc", 5), "abc");
        assert_eq!(head("abcdef", 3), "abc\n[…truncated]");
    }

    #[test]
    fn walk_skips_hidden_and_dependency_folders_and_caps_depth() {
        let root = temp_root();
        write(&root, ".git/config", "");
        write(&root, "node_modules/x/index.js", "");
        write(&root, "main.py", "");
        write(&root, "a/b/c/deep.py", "");
        write(&root, "a/b/c/d/too_deep.py", "");
        let mut files = walk(&root);
        files.sort();
        assert_eq!(files, vec!["a/b/c/deep.py", "main.py"]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn entrypoints_prefer_shallow_training_scripts() {
        let files: Vec<String> = [
            "archive/eval_countdown.py",
            "es/train.py",
            "main.py",
            "evaluate.py",
            "src/deep/nested/train.py",
            "trainer_notes.py",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_eq!(
            entrypoints(&files),
            vec!["main.py", "evaluate.py", "es/train.py"]
        );
    }

    #[test]
    fn parse_prompts_tolerates_fences_and_rejects_short_lists() {
        let raw = "Here you go:\n```json\n[\n {\"title\": \"Read  the\\tpaper\", \"prompt\": \" Summarize SWA. \"},\n {\"title\": \"b\", \"prompt\": \"2\"},\n {\"title\": \"c\", \"prompt\": \"3\"},\n {\"title\": \"d\", \"prompt\": \"4\"},\n {\"title\": \"e\", \"prompt\": \"5\"}\n]\n```";
        let prompts = parse_prompts(raw).unwrap();
        assert_eq!(prompts.len(), 4);
        assert_eq!(prompts[0].title, "Read the paper");
        assert_eq!(prompts[0].prompt, "Summarize SWA.");
        let long = format!(
            "[{{\"title\": \"{}\", \"prompt\": \"{}\"}}, {{\"title\": \"b\", \"prompt\": \"2\"}}, {{\"title\": \"c\", \"prompt\": \"3\"}}, {{\"title\": \"d\", \"prompt\": \"4\"}}]",
            "t".repeat(100),
            "p".repeat(1000)
        );
        let clipped = parse_prompts(&long).unwrap();
        assert_eq!(clipped[0].title.chars().count(), STARTER_TITLE_CHARS);
        assert_eq!(clipped[0].prompt.chars().count(), STARTER_PROMPT_CHARS);
        assert_eq!(
            parse_prompts("[{\"title\": \"a\", \"prompt\": \"1\"}]"),
            None
        );
        assert_eq!(parse_prompts("no json here"), None);
        let blank_title = "[{\"title\": \"\", \"prompt\": \"1\"}, {\"title\": \"b\", \"prompt\": \"2\"}, {\"title\": \"c\", \"prompt\": \"3\"}, {\"title\": \"d\", \"prompt\": \"4\"}]";
        assert_eq!(parse_prompts(blank_title), None);
    }

    #[test]
    fn prewarm_brief_matches_the_created_project() {
        // A blank project: nothing on disk once created.
        let root = temp_root();
        let mut project = project(&root);
        project.name = "Fresh idea".into();
        project.run_command = None;
        let created = brief(&project);
        let ahead = brief_parts("Fresh idea", None, None, Path::new(""), &[]);
        assert_eq!(created, ahead);
        // A paper project without a repository: just the PDF.
        write(&root, "paper.pdf", "%PDF");
        project.paper_id = Some("2401.12345".into());
        let created = brief(&project);
        let ahead = brief_parts(
            "Fresh idea",
            None,
            Some("2401.12345"),
            Path::new(""),
            &["paper.pdf".to_string()],
        );
        assert_eq!(created, ahead);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fingerprint_changes_with_brief_harness_and_locale() {
        let base = fingerprint("brief", "claude-code", "en");
        assert_ne!(base, fingerprint("brief2", "claude-code", "en"));
        assert_ne!(base, fingerprint("brief", "codex", "en"));
        assert_ne!(base, fingerprint("brief", "claude-code", "fa"));
        assert_eq!(base, fingerprint("brief", "claude-code", "en"));
    }
}
