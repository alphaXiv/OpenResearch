//! User-uploaded agent skills — the dashboard's Customize tab.
//!
//! Unlike the built-in `orx-*` modules in [`super::agent_skills`] (embedded in
//! the binary), these are authored by the user and stored on disk under the
//! data dir:
//!
//! * **Global** — `data_dir()/user-skills/global/<name>/` — every project.
//! * **Project** — `data_dir()/user-skills/projects/<project_id>/<name>/` — one
//!   project only; shadows a global of the same name for that project.
//!
//! Each skill is a real skill folder (`SKILL.md` plus any supporting files),
//! written into every session worktree's skills dir alongside the built-ins
//! (see [`write_into_session`]) so the harness auto-discovers it, and surfaced
//! in the composer's `/` menu so the user can invoke it by name (see
//! [`expand`]). The `SKILL.md` frontmatter's `name:` is the canonical id — it
//! is the skill dir name and the `/name` the user types.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{anyhow, Result};
use crate::local::agent_skills::SkillSet;
use crate::local::harness::registry;

/// Where an uploaded skill applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Available in every project's sessions.
    Global,
    /// Available only in one project's sessions.
    Project,
}

/// Reject pathological archives: a skill is a handful of small text files, not
/// a tarball. Caps guard the extract path against zip bombs (the composer also
/// caps the upload size client-side).
const MAX_FILES: usize = 500;
const MAX_TOTAL_BYTES: u64 = 20 * 1024 * 1024;
const MAX_NAME_LEN: usize = 64;
const MAX_DESCRIPTION_LEN: usize = 2048;

/// One uploaded skill, as served to the UI.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserSkill {
    pub name: String,
    pub description: String,
    pub scope: Scope,
    /// Total size of the skill folder on disk.
    pub bytes: u64,
    /// `SKILL.md` mtime in epoch millis (0 if unavailable).
    pub updated_at: i64,
}

// --- storage layout -----------------------------------------------------------

fn root() -> PathBuf {
    crate::store::data_dir().join("user-skills")
}

/// The directory holding a scope's skill folders under `root`. Validates
/// `project_id` so a crafted id can never escape the store root.
pub(crate) fn scope_dir(root: &Path, scope: Scope, project_id: Option<&str>) -> Result<PathBuf> {
    match scope {
        Scope::Global => Ok(root.join("global")),
        Scope::Project => {
            let id = project_id.ok_or_else(|| anyhow!("project scope requires a project id"))?;
            if !is_valid_project_id(id) {
                return Err(anyhow!("invalid project id"));
            }
            Ok(root.join("projects").join(id))
        }
    }
}

fn is_valid_project_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

// --- frontmatter --------------------------------------------------------------

struct Frontmatter {
    name: String,
    description: String,
}

/// Parse the `name:`/`description:` from a `SKILL.md` YAML frontmatter block.
/// Deliberately minimal (single-line scalars, quoted or bare) — the same shape
/// the built-in skills use and the shape Claude Code / Codex require.
fn parse_frontmatter(content: &str) -> Result<Frontmatter> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let after_open = content
        .strip_prefix("---")
        .and_then(|r| r.strip_prefix('\n').or_else(|| r.strip_prefix("\r\n")))
        .ok_or_else(|| anyhow!("SKILL.md must open with a `---` frontmatter block"))?;
    let end = after_open
        .find("\n---")
        .ok_or_else(|| anyhow!("SKILL.md frontmatter is not closed with `---`"))?;
    let block = &after_open[..end];

    let mut name = None;
    let mut description = None;
    for line in block.lines() {
        if let Some(v) = line.strip_prefix("name:") {
            name = Some(parse_scalar(v.trim()));
        } else if let Some(v) = line.strip_prefix("description:") {
            description = Some(parse_scalar(v.trim()));
        }
    }

    let name = name
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("SKILL.md frontmatter is missing a `name:` field"))?;
    let description = description
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("SKILL.md frontmatter is missing a `description:` field"))?;
    if description.chars().count() > MAX_DESCRIPTION_LEN {
        return Err(anyhow!("SKILL.md `description` is too long"));
    }
    Ok(Frontmatter { name, description })
}

/// Decode a single-line YAML scalar: JSON/double-quoted, single-quoted, or bare.
fn parse_scalar(v: &str) -> String {
    if v.starts_with('"') {
        if let Ok(s) = serde_json::from_str::<String>(v) {
            return s;
        }
    }
    if v.len() >= 2 && v.starts_with('\'') && v.ends_with('\'') {
        return v[1..v.len() - 1].replace("''", "'");
    }
    v.to_string()
}

// --- name validation ----------------------------------------------------------

/// Slug rule shared with the built-in skills: `^[a-z0-9]+(-[a-z0-9]+)*$`. This
/// is also what the harnesses require of a skill `name`.
pub(crate) fn is_valid_slug(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_LEN
        && name.split('-').all(|seg| {
            !seg.is_empty()
                && seg
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

/// Names owned by the built-ins: the `orx-` namespace, any bundled agent skill
/// (bare or prefixed), and the composer's slash-skill catalog. Rejected so an
/// upload can never shadow or be shadowed by a built-in.
fn is_reserved(name: &str) -> bool {
    name == "plan"
        || name.starts_with("orx-")
        || crate::local::agent_skills::find(name, SkillSet::Full).is_some()
        || crate::local::agent_skills::find(name, SkillSet::Local).is_some()
        || crate::local::skills::CATALOG.iter().any(|s| s.name == name)
}

fn validate_name(name: &str) -> Result<()> {
    if !is_valid_slug(name) {
        return Err(anyhow!(
            "skill name `{name}` must be lowercase letters, digits, and single hyphens"
        ));
    }
    if is_reserved(name) {
        return Err(anyhow!("`{name}` is reserved by a built-in skill"));
    }
    Ok(())
}

// --- save ---------------------------------------------------------------------

/// Save a single-file `SKILL.md` upload. The name comes from its frontmatter.
pub fn save_skill_md(content: &[u8], scope: Scope, project_id: Option<&str>) -> Result<UserSkill> {
    save_skill_md_in(&root(), content, scope, project_id)
}

fn save_skill_md_in(
    root: &Path,
    content: &[u8],
    scope: Scope,
    project_id: Option<&str>,
) -> Result<UserSkill> {
    let text = std::str::from_utf8(content).map_err(|_| anyhow!("SKILL.md must be UTF-8 text"))?;
    let fm = parse_frontmatter(text)?;
    validate_name(&fm.name)?;
    write_skill(
        root,
        scope,
        project_id,
        &fm.name,
        vec![("SKILL.md".to_string(), content.to_vec())],
    )
}

/// Save a `.zip` of a skill folder. Locates the `SKILL.md` (root or a single
/// wrapping dir), rebases every sibling file beneath it, and writes the folder
/// verbatim. Rejects unsafe paths, oversized archives, and a missing/ambiguous
/// `SKILL.md`.
pub fn save_zip(bytes: &[u8], scope: Scope, project_id: Option<&str>) -> Result<UserSkill> {
    save_zip_in(&root(), bytes, scope, project_id)
}

fn save_zip_in(
    root: &Path,
    bytes: &[u8],
    scope: Scope,
    project_id: Option<&str>,
) -> Result<UserSkill> {
    use std::io::{Cursor, Read};

    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| anyhow!("not a valid .zip archive: {e}"))?;

    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut skill_md: Option<String> = None;
    let mut total: u64 = 0;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| anyhow!("reading zip entry: {e}"))?;
        if entry.is_dir() {
            continue;
        }
        // `enclosed_name` returns None for absolute paths and `..` escapes.
        let name = entry
            .enclosed_name()
            .ok_or_else(|| anyhow!("zip contains an unsafe path"))?
            .to_string_lossy()
            .replace('\\', "/");
        if name.starts_with("__MACOSX/") || basename(&name) == ".DS_Store" {
            continue;
        }
        if files.len() >= MAX_FILES {
            return Err(anyhow!("zip has too many files (max {MAX_FILES})"));
        }
        // Bound the read to the remaining budget so a deflate-bombed entry
        // (whose declared size can't be trusted) can't balloon `buf`.
        let mut buf = Vec::new();
        let room = MAX_TOTAL_BYTES.saturating_sub(total);
        (&mut entry)
            .take(room + 1)
            .read_to_end(&mut buf)
            .map_err(|e| anyhow!("reading zip entry {name}: {e}"))?;
        total = total.saturating_add(buf.len() as u64);
        if total > MAX_TOTAL_BYTES {
            return Err(anyhow!("zip is too large once extracted"));
        }
        if basename(&name) == "SKILL.md" {
            // Prefer the shallowest SKILL.md if several appear.
            if skill_md
                .as_deref()
                .is_none_or(|existing| depth(&name) < depth(existing))
            {
                skill_md = Some(name.clone());
            }
        }
        files.push((name, buf));
    }

    let skill_md = skill_md.ok_or_else(|| anyhow!("the .zip must contain a SKILL.md"))?;
    let prefix = match skill_md.rsplit_once('/') {
        Some((dir, _)) => format!("{dir}/"),
        None => String::new(),
    };

    let md = files
        .iter()
        .find(|(n, _)| *n == skill_md)
        .map(|(_, b)| b.clone())
        .unwrap_or_default();
    let text = std::str::from_utf8(&md).map_err(|_| anyhow!("SKILL.md must be UTF-8 text"))?;
    let fm = parse_frontmatter(text)?;
    validate_name(&fm.name)?;

    let mut rebased: Vec<(String, Vec<u8>)> = Vec::new();
    for (name, buf) in files {
        let Some(rel) = name.strip_prefix(&prefix) else {
            continue; // a stray file outside the skill folder — ignore
        };
        if rel.is_empty()
            || rel
                .split('/')
                .any(|seg| seg.is_empty() || seg == "." || seg == "..")
        {
            return Err(anyhow!("zip contains an unsafe path: {name}"));
        }
        rebased.push((rel.to_string(), buf));
    }

    write_skill(root, scope, project_id, &fm.name, rebased)
}

fn write_skill(
    root: &Path,
    scope: Scope,
    project_id: Option<&str>,
    name: &str,
    files: Vec<(String, Vec<u8>)>,
) -> Result<UserSkill> {
    if !files.iter().any(|(rel, _)| rel == "SKILL.md") {
        return Err(anyhow!("skill folder must contain a SKILL.md at its root"));
    }
    let dir = scope_dir(root, scope, project_id)?.join(name);
    // A re-upload fully replaces the prior version — no stale sibling files.
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|e| anyhow!("could not replace skill: {e}"))?;
    }
    for (rel, buf) in &files {
        let dest = dir.join(rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| anyhow!("could not create {}: {e}", parent.display()))?;
        }
        fs::write(&dest, buf).map_err(|e| anyhow!("could not write {}: {e}", dest.display()))?;
    }
    read_skill_at(scope, &dir)
}

// --- list / find / delete -----------------------------------------------------

fn read_skill_at(scope: Scope, dir: &Path) -> Result<UserSkill> {
    let md_path = dir.join("SKILL.md");
    let content = fs::read_to_string(&md_path)
        .map_err(|e| anyhow!("could not read {}: {e}", md_path.display()))?;
    let fm = parse_frontmatter(&content)?;
    Ok(UserSkill {
        name: fm.name,
        description: fm.description,
        scope,
        bytes: dir_size(dir),
        updated_at: mtime_ms(&md_path),
    })
}

/// Skills in one scope, name-sorted. Unreadable/malformed folders are skipped
/// rather than failing the whole listing.
fn list_scope_in(root: &Path, scope: Scope, project_id: Option<&str>) -> Vec<UserSkill> {
    let Ok(base) = scope_dir(root, scope, project_id) else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(&base) else {
        return Vec::new();
    };
    let mut out: Vec<UserSkill> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| read_skill_at(scope, &e.path()).ok())
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Every skill that applies to a project's sessions: globals plus the project's
/// own. Both scopes are returned (the UI manages each separately); a project
/// skill and a global of the same name both appear.
pub fn list_for_project(project_id: Option<&str>) -> Vec<UserSkill> {
    let root = root();
    let mut out = list_scope_in(&root, Scope::Global, None);
    if let Some(id) = project_id {
        out.extend(list_scope_in(&root, Scope::Project, Some(id)));
    }
    out
}

/// Resolve an applicable skill by name for `/`-invocation. Project scope wins
/// over global.
fn find_in(root: &Path, name: &str, project_id: Option<&str>) -> Option<UserSkill> {
    if let Some(id) = project_id {
        if let Some(s) = list_scope_in(root, Scope::Project, Some(id))
            .into_iter()
            .find(|s| s.name == name)
        {
            return Some(s);
        }
    }
    list_scope_in(root, Scope::Global, None)
        .into_iter()
        .find(|s| s.name == name)
}

/// The Markdown body of the applicable skill's `SKILL.md`, for the composer
/// hover preview. Project scope wins over global just as invocation does.
pub fn content(name: &str, project_id: Option<&str>) -> Option<String> {
    content_in(&root(), name, project_id)
}

fn content_in(root: &Path, name: &str, project_id: Option<&str>) -> Option<String> {
    let skill = find_in(root, name, project_id)?;
    let scope_project = if skill.scope == Scope::Project {
        Some(project_id?)
    } else {
        None
    };
    let path = scope_dir(root, skill.scope, scope_project)
        .ok()?
        .join(&skill.name)
        .join("SKILL.md");
    let content = fs::read_to_string(path).ok()?;
    let content = content.strip_prefix('\u{feff}').unwrap_or(&content);
    let after_open = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))?;
    let end = after_open.find("\n---")?;
    Some(
        after_open[end + 4..]
            .trim_start_matches(['\r', '\n'])
            .to_string(),
    )
}

pub fn delete(name: &str, scope: Scope, project_id: Option<&str>) -> Result<()> {
    delete_in(&root(), name, scope, project_id)
}

fn delete_in(root: &Path, name: &str, scope: Scope, project_id: Option<&str>) -> Result<()> {
    if !is_valid_slug(name) {
        return Err(anyhow!("invalid skill name"));
    }
    let dir = scope_dir(root, scope, project_id)?.join(name);
    if !dir.exists() {
        return Err(anyhow!("skill `{name}` not found"));
    }
    fs::remove_dir_all(&dir).map_err(|e| anyhow!("could not delete skill: {e}"))
}

// --- import from a coding agent -----------------------------------------------

/// A skill already installed in a coding agent, offered for one-click import.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessSkill {
    pub harness_id: String,
    pub harness_name: String,
    pub name: String,
    pub description: String,
}

/// Every importable skill across installed harnesses (their global skill dirs),
/// minus the `orx` shim, the built-in namespace, and anything malformed or
/// name-reserved. Name-sorted within each harness.
pub fn list_harness_skills() -> Vec<HarnessSkill> {
    let mut out = Vec::new();
    for harness in registry() {
        if !harness.is_installed_locally() {
            continue;
        }
        let Some(dir) = harness.global_skills_dir() else {
            continue;
        };
        let Ok(entries) = fs::read_dir(&dir) else {
            continue; // agent installed but no skills dir yet
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let dir_name = entry.file_name().to_string_lossy().into_owned();
            if dir_name == "orx" || dir_name.starts_with("orx-") {
                continue;
            }
            let Ok(content) = fs::read_to_string(path.join("SKILL.md")) else {
                continue; // not a skill dir
            };
            let Ok(fm) = parse_frontmatter(&content) else {
                continue;
            };
            // Import keys off the frontmatter name (it becomes the store dir and
            // the `/name`); skip skills whose name isn't a clean, free slug so
            // the folder==name invariant always holds after import.
            let name = fm.name;
            if !is_valid_slug(&name) || is_reserved(&name) {
                continue;
            }
            out.push(HarnessSkill {
                harness_id: harness.id().to_string(),
                harness_name: harness.name().to_string(),
                name,
                description: fm.description,
            });
        }
    }
    out.sort_by(|a, b| {
        (a.harness_name.as_str(), a.name.as_str()).cmp(&(b.harness_name.as_str(), b.name.as_str()))
    });
    out
}

/// Copy a named skill from an installed harness's global skills dir into the
/// store at `scope`, resources and all.
pub fn import_from_harness(
    harness_id: &str,
    name: &str,
    scope: Scope,
    project_id: Option<&str>,
) -> Result<UserSkill> {
    import_from_harness_in(&root(), harness_id, name, scope, project_id)
}

fn import_from_harness_in(
    root: &Path,
    harness_id: &str,
    name: &str,
    scope: Scope,
    project_id: Option<&str>,
) -> Result<UserSkill> {
    validate_name(name)?;
    let harness = registry()
        .into_iter()
        .find(|h| h.id() == harness_id)
        .ok_or_else(|| anyhow!("unknown harness `{harness_id}`"))?;
    let skills_dir = harness
        .global_skills_dir()
        .ok_or_else(|| anyhow!("{} has no skills directory", harness.name()))?;
    let src = locate_harness_skill(&skills_dir, name)?;
    let files = collect_files(&src, &src)?;
    write_skill(root, scope, project_id, name, files)
}

/// The skill folder in `skills_dir` whose `SKILL.md` frontmatter name matches
/// `name` — the same key `list_harness_skills` surfaces, so the folder stored on
/// import always equals its frontmatter name.
fn locate_harness_skill(skills_dir: &Path, name: &str) -> Result<PathBuf> {
    let entries = fs::read_dir(skills_dir)
        .map_err(|e| anyhow!("could not read {}: {e}", skills_dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Ok(content) = fs::read_to_string(path.join("SKILL.md")) else {
            continue;
        };
        let Ok(fm) = parse_frontmatter(&content) else {
            continue;
        };
        if fm.name == name {
            return Ok(path);
        }
    }
    Err(anyhow!("skill `{name}` not found in the harness"))
}

/// Every file under `dir`, as (base-relative path, bytes).
fn collect_files(base: &Path, dir: &Path) -> Result<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| anyhow!("could not read {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| anyhow!("could not read dir entry: {e}"))?;
        let path = entry.path();
        if path.is_dir() {
            out.extend(collect_files(base, &path)?);
        } else {
            let rel = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let bytes =
                fs::read(&path).map_err(|e| anyhow!("could not read {}: {e}", path.display()))?;
            out.push((rel, bytes));
        }
    }
    Ok(out)
}

// --- session wiring -----------------------------------------------------------

/// A manifest of the skill dirs [`write_into_session`] wrote last turn, so it can
/// prune ones the user has since deleted without touching built-in `orx-*` or
/// repo-committed skills. Lives inside the (git-excluded) session skills dir; a
/// dotfile, so the harness never treats it as a skill.
const MANAGED_MANIFEST: &str = ".orx-user-skills";

/// Copy every applicable skill folder (globals first, then the project's, so a
/// project skill shadows a global of the same name) into the session worktree's
/// native skills dir, beside the built-in `orx-*` skills. Called fresh each turn
/// from `ensure_playbook`: each managed dir is fully replaced, and any skill the
/// user deleted since last turn is removed — so a session's skills track the
/// store with no drift.
pub fn write_into_session(worktree: &Path, skills_dir_rel: &str, project_id: &str) -> Result<()> {
    write_into_session_in(&root(), worktree, skills_dir_rel, project_id)
}

fn write_into_session_in(
    root: &Path,
    worktree: &Path,
    skills_dir_rel: &str,
    project_id: &str,
) -> Result<()> {
    let base = worktree.join(skills_dir_rel);
    let mut managed: Vec<String> = Vec::new();
    for (scope, pid) in [(Scope::Global, None), (Scope::Project, Some(project_id))] {
        let Ok(src_base) = scope_dir(root, scope, pid) else {
            continue;
        };
        let Ok(entries) = fs::read_dir(&src_base) else {
            continue; // no skills for this scope yet
        };
        for entry in entries.flatten() {
            let src = entry.path();
            if !src.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let dest = base.join(&name);
            // Replace, don't merge: a re-upload with fewer files, or a project
            // skill shadowing a global, must not keep stale siblings.
            if dest.exists() {
                let _ = fs::remove_dir_all(&dest);
            }
            copy_dir_all(&src, &dest)?;
            if !managed.contains(&name) {
                managed.push(name);
            }
        }
    }
    // Prune skills we wrote before but that no longer apply (deleted or renamed).
    if let Ok(prev) = fs::read_to_string(base.join(MANAGED_MANIFEST)) {
        for name in prev.lines().filter(|n| !n.is_empty()) {
            if !managed.iter().any(|m| m == name) {
                let _ = fs::remove_dir_all(base.join(name));
            }
        }
    }
    if base.exists() {
        let _ = fs::write(base.join(MANAGED_MANIFEST), managed.join("\n"));
    }
    Ok(())
}

/// Build the instruction for one selected user skill. Its complete `SKILL.md`
/// is already in the worktree; the chat layer supplies the shared user request.
pub fn instructions(name: &str, project_id: &str) -> Option<String> {
    instructions_in(&root(), name, project_id)
}

fn instructions_in(root: &Path, name: &str, project_id: &str) -> Option<String> {
    let skill = find_in(root, name, Some(project_id))?;
    Some(format!("Use the `{}` skill.", skill.name))
}

// --- fs helpers ---------------------------------------------------------------

pub(crate) fn copy_dir_all(src: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest).map_err(|e| anyhow!("could not create {}: {e}", dest.display()))?;
    for entry in fs::read_dir(src).map_err(|e| anyhow!("could not read {}: {e}", src.display()))? {
        let entry = entry.map_err(|e| anyhow!("could not read dir entry: {e}"))?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if from.is_dir() {
            copy_dir_all(&from, &to)?;
        } else {
            fs::copy(&from, &to).map_err(|e| anyhow!("could not copy {}: {e}", from.display()))?;
        }
    }
    Ok(())
}

pub(crate) fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            total += dir_size(&path);
        } else if let Ok(meta) = entry.metadata() {
            total += meta.len();
        }
    }
    total
}

pub(crate) fn mtime_ms(path: &Path) -> i64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub(crate) fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

pub(crate) fn depth(path: &str) -> usize {
    path.matches('/').count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> PathBuf {
        std::env::temp_dir().join(format!("orx-user-skills-test-{}", uuid::Uuid::new_v4()))
    }

    fn skill_md(name: &str) -> String {
        skill_md_desc(name, &format!("A test skill for {name}. Use when testing."))
    }

    fn skill_md_desc(name: &str, desc: &str) -> String {
        format!("---\nname: {name}\ndescription: {desc}\n---\n\n# {name}\nbody\n")
    }

    fn make_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default();
            for (name, data) in entries {
                w.start_file(*name, opts).unwrap();
                w.write_all(data).unwrap();
            }
            w.finish().unwrap();
        }
        buf
    }

    #[test]
    fn frontmatter_parsing() {
        let fm = parse_frontmatter("---\nname: foo\ndescription: hi there\n---\n\nbody").unwrap();
        assert_eq!(fm.name, "foo");
        assert_eq!(fm.description, "hi there");
        // A quoted scalar keeps an embedded colon.
        let fm = parse_frontmatter("---\nname: foo\ndescription: \"a: b\"\n---\nbody").unwrap();
        assert_eq!(fm.description, "a: b");
        assert!(parse_frontmatter("no frontmatter here").is_err());
        assert!(parse_frontmatter("---\ndescription: x\n---\nb").is_err());
        assert!(parse_frontmatter("---\nname: x\n---\nb").is_err());
    }

    #[test]
    fn name_validation() {
        assert!(is_valid_slug("data-cleaner"));
        assert!(is_valid_slug("skill1"));
        assert!(!is_valid_slug("Bad_Name"));
        assert!(!is_valid_slug("has space"));
        assert!(!is_valid_slug("-leading"));
        assert!(!is_valid_slug("double--hyphen"));
        // Built-in namespaces are reserved (bare and prefixed forms).
        assert!(is_reserved("orx-git"));
        assert!(is_reserved("compute"));
        assert!(is_reserved("lit-review"));
        assert!(is_reserved("plan"));
        assert!(!is_reserved("haiku-writer"));
    }

    #[test]
    fn save_list_find_delete_roundtrip() {
        let root = temp_root();
        let saved =
            save_skill_md_in(&root, skill_md("alpha").as_bytes(), Scope::Global, None).unwrap();
        assert_eq!(saved.name, "alpha");
        assert_eq!(saved.scope, Scope::Global);
        assert!(root.join("global/alpha/SKILL.md").exists());

        let list = list_scope_in(&root, Scope::Global, None);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "alpha");
        assert!(find_in(&root, "alpha", None).is_some());

        delete_in(&root, "alpha", Scope::Global, None).unwrap();
        assert!(list_scope_in(&root, Scope::Global, None).is_empty());
        // Deleting a missing skill is an error.
        assert!(delete_in(&root, "alpha", Scope::Global, None).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_reserved_and_bad_names() {
        let root = temp_root();
        assert!(save_skill_md_in(
            &root,
            skill_md("orx-compute").as_bytes(),
            Scope::Global,
            None
        )
        .is_err());
        assert!(save_skill_md_in(
            &root,
            skill_md("lit-review").as_bytes(),
            Scope::Global,
            None
        )
        .is_err());
        let bad = "---\nname: Bad_Name\ndescription: nope. Use.\n---\nbody\n";
        assert!(save_skill_md_in(&root, bad.as_bytes(), Scope::Global, None).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn save_zip_preserves_folder() {
        let root = temp_root();
        let md = skill_md("packager");
        let zip = make_zip(&[
            ("packager/SKILL.md", md.as_bytes()),
            ("packager/scripts/run.py", b"print(1)\n"),
            ("__MACOSX/packager/._SKILL.md", b"junk"),
        ]);
        let saved = save_zip_in(&root, &zip, Scope::Global, None).unwrap();
        assert_eq!(saved.name, "packager");
        assert!(root.join("global/packager/SKILL.md").exists());
        assert!(root.join("global/packager/scripts/run.py").exists());
        // The __MACOSX junk was skipped, not written under the skill.
        assert!(!root.join("global/packager/__MACOSX").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn save_zip_requires_a_skill_md() {
        let root = temp_root();
        let zip = make_zip(&[("foo/readme.txt", b"hi")]);
        assert!(save_zip_in(&root, &zip, Scope::Global, None).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn re_upload_replaces_prior_files() {
        let root = temp_root();
        let zip = make_zip(&[
            ("s/SKILL.md", skill_md("s").as_bytes()),
            ("s/old.txt", b"stale"),
        ]);
        save_zip_in(&root, &zip, Scope::Global, None).unwrap();
        assert!(root.join("global/s/old.txt").exists());
        // Re-upload without old.txt must drop it.
        save_skill_md_in(&root, skill_md("s").as_bytes(), Scope::Global, None).unwrap();
        assert!(!root.join("global/s/old.txt").exists());
        assert!(root.join("global/s/SKILL.md").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn project_scope_shadows_global() {
        let root = temp_root();
        save_skill_md_in(
            &root,
            skill_md_desc("dup", "GLOBAL body").as_bytes(),
            Scope::Global,
            None,
        )
        .unwrap();
        save_skill_md_in(
            &root,
            skill_md_desc("dup", "PROJECT body").as_bytes(),
            Scope::Project,
            Some("proj1"),
        )
        .unwrap();

        let found = find_in(&root, "dup", Some("proj1")).unwrap();
        assert_eq!(found.scope, Scope::Project);
        assert!(found.description.contains("PROJECT"));
        // A different project falls back to the global.
        let other = find_in(&root, "dup", Some("proj2")).unwrap();
        assert_eq!(other.scope, Scope::Global);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn write_into_session_copies_scopes_with_project_shadow() {
        let root = temp_root();
        save_skill_md_in(&root, skill_md("g-only").as_bytes(), Scope::Global, None).unwrap();
        save_skill_md_in(
            &root,
            skill_md_desc("dup", "GLOBAL").as_bytes(),
            Scope::Global,
            None,
        )
        .unwrap();
        save_skill_md_in(
            &root,
            skill_md_desc("dup", "PROJECT").as_bytes(),
            Scope::Project,
            Some("p1"),
        )
        .unwrap();

        let wt = temp_root();
        write_into_session_in(&root, &wt, ".claude/skills", "p1").unwrap();
        assert!(wt.join(".claude/skills/g-only/SKILL.md").exists());
        let dup = fs::read_to_string(wt.join(".claude/skills/dup/SKILL.md")).unwrap();
        assert!(
            dup.contains("PROJECT"),
            "project skill must shadow the global"
        );
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&wt);
    }

    #[test]
    fn instructions_invoke_user_skill() {
        let root = temp_root();
        save_skill_md_in(&root, skill_md("greeter").as_bytes(), Scope::Global, None).unwrap();
        assert_eq!(
            instructions_in(&root, "greeter", "p1").unwrap(),
            "Use the `greeter` skill."
        );
        assert_eq!(
            content_in(&root, "greeter", Some("p1")).unwrap(),
            "# greeter\nbody\n"
        );
        assert!(content_in(&root, "../../etc/passwd", Some("p1")).is_none());
        assert!(instructions_in(&root, "unknown", "p1").is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn collect_files_walks_nested_dirs() {
        let base = temp_root();
        fs::create_dir_all(base.join("scripts")).unwrap();
        fs::write(base.join("SKILL.md"), b"x").unwrap();
        fs::write(base.join("scripts/run.py"), b"y").unwrap();
        let mut files = collect_files(&base, &base).unwrap();
        files.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].0, "SKILL.md");
        assert_eq!(files[1].0, "scripts/run.py");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn locate_harness_skill_matches_frontmatter_name() {
        let skills = temp_root();
        // Match on the frontmatter name even when the dir name differs, and
        // never on the dir name alone.
        fs::create_dir_all(skills.join("weird-dir")).unwrap();
        fs::write(skills.join("weird-dir/SKILL.md"), skill_md("canonical")).unwrap();

        assert_eq!(
            locate_harness_skill(&skills, "canonical").unwrap(),
            skills.join("weird-dir")
        );
        assert!(locate_harness_skill(&skills, "weird-dir").is_err());
        assert!(locate_harness_skill(&skills, "missing").is_err());
        let _ = fs::remove_dir_all(&skills);
    }

    #[test]
    fn write_into_session_prunes_deleted_skills() {
        let root = temp_root();
        let wt = temp_root();
        let rel = ".claude/skills";
        save_skill_md_in(&root, skill_md("keep").as_bytes(), Scope::Global, None).unwrap();
        save_skill_md_in(&root, skill_md("gone").as_bytes(), Scope::Global, None).unwrap();
        write_into_session_in(&root, &wt, rel, "p1").unwrap();
        assert!(wt.join(rel).join("keep/SKILL.md").exists());
        assert!(wt.join(rel).join("gone/SKILL.md").exists());

        // Delete one skill from the store, re-run: it must leave the worktree.
        delete_in(&root, "gone", Scope::Global, None).unwrap();
        write_into_session_in(&root, &wt, rel, "p1").unwrap();
        assert!(wt.join(rel).join("keep/SKILL.md").exists());
        assert!(
            !wt.join(rel).join("gone").exists(),
            "deleted skill must be pruned"
        );
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&wt);
    }

    #[test]
    fn write_into_session_shadow_replaces_not_merges() {
        let root = temp_root();
        let wt = temp_root();
        let rel = ".agents/skills";
        // Global `dup` carries an extra file the project version lacks.
        write_skill(
            &root,
            Scope::Global,
            None,
            "dup",
            vec![
                (
                    "SKILL.md".into(),
                    skill_md_desc("dup", "GLOBAL").into_bytes(),
                ),
                ("extra.txt".into(), b"global-only".to_vec()),
            ],
        )
        .unwrap();
        save_skill_md_in(
            &root,
            skill_md_desc("dup", "PROJECT").as_bytes(),
            Scope::Project,
            Some("p1"),
        )
        .unwrap();
        write_into_session_in(&root, &wt, rel, "p1").unwrap();
        let md = fs::read_to_string(wt.join(rel).join("dup/SKILL.md")).unwrap();
        assert!(md.contains("PROJECT"));
        assert!(
            !wt.join(rel).join("dup/extra.txt").exists(),
            "shadowing project skill must not keep the global's files"
        );
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&wt);
    }

    #[test]
    fn save_zip_rejects_oversized_and_too_many_files() {
        let root = temp_root();
        // One entry whose declared uncompressed size exceeds the cap (zeros
        // compress tiny, so the archive itself stays small).
        let big = make_zip(&[
            ("s/SKILL.md", skill_md("s").as_bytes()),
            ("s/big.bin", &vec![0u8; (MAX_TOTAL_BYTES + 1) as usize]),
        ]);
        assert!(save_zip_in(&root, &big, Scope::Global, None).is_err());

        let skill = skill_md("s");
        let many: Vec<(String, Vec<u8>)> = (0..MAX_FILES + 1)
            .map(|i| (format!("s/f{i}.txt"), b"x".to_vec()))
            .collect();
        let mut entries: Vec<(&str, &[u8])> = vec![("s/SKILL.md", skill.as_bytes())];
        for (n, b) in &many {
            entries.push((n.as_str(), b.as_slice()));
        }
        assert!(save_zip_in(&root, &make_zip(&entries), Scope::Global, None).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn quoted_frontmatter_name_is_accepted() {
        let root = temp_root();
        let md =
            "---\nname: \"data-cleaner\"\ndescription: Clean CSVs. Use when asked.\n---\nbody\n";
        let saved = save_skill_md_in(&root, md.as_bytes(), Scope::Global, None).unwrap();
        assert_eq!(saved.name, "data-cleaner");
        let _ = fs::remove_dir_all(&root);
    }
}
