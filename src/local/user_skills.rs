//! The skills the agent gets in every session — the dashboard's Customize tab.
//!
//! Unlike the built-in `orx-*` modules in [`super::agent_skills`] (embedded in
//! the binary), these come from the user, from two sources that share one flat
//! list:
//!
//! * **Uploaded** — `data_dir()/user-skills/global/<name>/`, added from the
//!   Customize tab.
//! * **Mirrored** — the skills dirs of the coding agents installed on this
//!   machine (`~/.claude/skills`, `~/.agents/skills`, …) plus the skills their
//!   installed plugins ship, read live so a skill the user edits in Claude Code
//!   or Codex is the one the next session runs. An upload of the same name
//!   shadows the mirrored copy.
//!
//! Each skill is a real skill folder (`SKILL.md` plus any supporting files),
//! written into every session worktree's skills dir alongside the built-ins (see
//! [`write_into_session`]) so the harness auto-discovers it, and surfaced in the
//! composer's `/` menu so the user can invoke it by name. The folder name is the
//! canonical id — the `/name` the user types and the dir written into a session;
//! an upload's `SKILL.md` frontmatter `name:` only seeds it.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{anyhow, Result};
use crate::local::agent_skills::SkillSet;
use crate::local::harness::registry;

/// Reject pathological archives: a skill is a handful of small text files, not
/// a tarball. Caps guard the extract path against zip bombs (the composer also
/// caps the upload size client-side).
const MAX_FILES: usize = 500;
const MAX_TOTAL_BYTES: u64 = 20 * 1024 * 1024;
const MAX_NAME_LEN: usize = 64;
const MAX_DESCRIPTION_LEN: usize = 2048;

/// One skill available to every session, as served to the UI.
#[derive(Clone, Debug)]
pub struct UserSkill {
    pub name: String,
    pub description: String,
    /// The coding agent or plugin this skill is mirrored from — `None` when
    /// uploaded here. A mirrored skill is read-only: it is managed where it
    /// lives.
    pub origin: Option<String>,
    /// Total size of the skill folder on disk.
    pub bytes: u64,
    /// `SKILL.md` mtime in epoch millis (0 if unavailable).
    pub updated_at: i64,
}

// --- storage layout -----------------------------------------------------------

/// Every entry path resolves the store through here, which is also where the
/// retired per-project layout gets folded in: after the first run that costs one
/// `is_dir`, and no other choke point covers the CLI and the dashboard alike.
fn root() -> PathBuf {
    let root = crate::store::data_dir().join("user-skills");
    migrate_project_scoped(&root);
    root
}

/// Uploaded skills all live in one directory — the store had a per-project scope
/// before, kept here as the on-disk name so existing installs need no move.
pub(crate) fn store_dir(root: &Path) -> PathBuf {
    root.join("global")
}

/// Lift what the retired per-project scope held into the single store. Anything
/// that can't move — a name the global store already uses, or a rename that
/// failed — stays exactly where it is, because it is the user's only copy; the
/// old tree is removed only once emptied, so a later run retries. Shared with
/// [`super::latex_templates`], whose store has the same shape.
pub(crate) fn migrate_project_scoped(root: &Path) {
    let projects = root.join("projects");
    if !projects.is_dir() {
        return;
    }
    let dest_base = store_dir(root);
    if let Ok(entries) = fs::read_dir(&projects) {
        for project in entries.flatten() {
            let Ok(items) = fs::read_dir(project.path()) else {
                continue;
            };
            for item in items.flatten() {
                // Junk keeps the dir non-empty, which would keep the retired
                // tree — and this walk — alive on every call forever.
                if item.file_name() == ".DS_Store" {
                    let _ = fs::remove_file(item.path());
                    continue;
                }
                let dest = dest_base.join(item.file_name());
                if dest.exists() || !item.path().is_dir() {
                    continue;
                }
                if fs::create_dir_all(&dest_base).is_ok() {
                    let _ = fs::rename(item.path(), &dest);
                }
            }
            // Non-recursive: succeeds only when everything moved out.
            let _ = fs::remove_dir(project.path());
        }
    }
    let _ = fs::remove_dir(&projects);
}

// --- frontmatter --------------------------------------------------------------

struct Frontmatter {
    name: String,
    description: String,
}

/// Parse the `name:`/`description:` from a `SKILL.md` YAML frontmatter block.
/// Covers the shapes skills are actually written in: a plain or quoted scalar,
/// a folded/literal block (`>-`, `|`), and a value continued on the following
/// indented lines. A skill whose frontmatter we can't read is a skill the user
/// never sees, so this errs towards reading it.
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
    let lines: Vec<&str> = block.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let Some((key, rest)) = top_level_key(lines[i]) else {
            i += 1;
            continue;
        };
        i += 1;
        if key != "name" && key != "description" {
            continue;
        }
        // Everything indented under the key (blank lines included) belongs to
        // its value; a line back at column 0 starts the next key, and a comment
        // there ends the value rather than joining it.
        let mut continuation: Vec<&str> = Vec::new();
        while i < lines.len() && top_level_key(lines[i]).is_none() && !is_comment(lines[i]) {
            continuation.push(lines[i].trim());
            i += 1;
        }
        let value = parse_scalar(&join_value(rest.trim(), &continuation));
        if key == "name" {
            name = Some(value);
        } else {
            description = Some(value);
        }
    }

    let name = name
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("SKILL.md frontmatter is missing a `name:` field"))?;
    let mut description = description
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("SKILL.md frontmatter is missing a `description:` field"))?;
    // Truncated rather than rejected: the cap bounds what we hold and show, and
    // a long description in a skill the user already installed elsewhere is no
    // reason to make that skill disappear.
    if description.chars().count() > MAX_DESCRIPTION_LEN {
        description = description.chars().take(MAX_DESCRIPTION_LEN).collect();
    }
    Ok(Frontmatter { name, description })
}

/// A comment at column 0. An indented `#` is content inside a block scalar.
fn is_comment(line: &str) -> bool {
    line.starts_with('#')
}

/// `("name", "the rest")` for a mapping key at column 0. Indented lines (a
/// value's continuation, or a nested mapping's own keys) return `None`.
fn top_level_key(line: &str) -> Option<(&str, &str)> {
    if line.starts_with(char::is_whitespace) || is_comment(line) {
        return None;
    }
    let (key, rest) = line.split_once(':')?;
    let key = key.trim();
    (!key.is_empty() && !key.contains(char::is_whitespace)).then_some((key, rest))
}

/// One value from its first line plus the indented lines under it. A `|` block
/// keeps its line breaks; everything else folds onto one line, which is how a
/// wrapped description was meant to read.
fn join_value(first: &str, continuation: &[&str]) -> String {
    if is_block_indicator(first, '|') {
        return continuation.join("\n").trim().to_string();
    }
    let folded = if is_block_indicator(first, '>') {
        ""
    } else {
        first
    };
    std::iter::once(folded)
        .chain(continuation.iter().copied())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A `|`/`>` block header, with only YAML's chomping and indent modifiers after
/// it — so a value that merely starts with the character (`>50 tests run.`) is
/// still read as text.
fn is_block_indicator(value: &str, indicator: char) -> bool {
    value.starts_with(indicator)
        && value[indicator.len_utf8()..]
            .chars()
            .all(|c| c == '-' || c == '+' || c.is_ascii_digit())
}

/// Decode a YAML scalar: JSON/double-quoted, single-quoted, or bare.
fn parse_scalar(v: &str) -> String {
    if v.starts_with('"') {
        if let Ok(s) = serde_json::from_str::<String>(v) {
            return s;
        }
        // A YAML double-quoted scalar JSON rejects (a stray escape, say) still
        // reads fine once unwrapped — better that than dropping the skill.
        if v.len() >= 2 && v.ends_with('"') {
            return v[1..v.len() - 1].replace("\\\"", "\"");
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
pub fn save_skill_md(content: &[u8]) -> Result<UserSkill> {
    save_skill_md_in(&root(), content)
}

fn save_skill_md_in(root: &Path, content: &[u8]) -> Result<UserSkill> {
    let text = std::str::from_utf8(content).map_err(|_| anyhow!("SKILL.md must be UTF-8 text"))?;
    let fm = parse_frontmatter(text)?;
    validate_name(&fm.name)?;
    write_skill(
        root,
        &fm.name,
        vec![("SKILL.md".to_string(), content.to_vec())],
    )
}

/// Save a `.zip` of a skill folder. Locates the `SKILL.md` (root or a single
/// wrapping dir), rebases every sibling file beneath it, and writes the folder
/// verbatim. Rejects unsafe paths, oversized archives, and a missing/ambiguous
/// `SKILL.md`.
pub fn save_zip(bytes: &[u8]) -> Result<UserSkill> {
    save_zip_in(&root(), bytes)
}

fn save_zip_in(root: &Path, bytes: &[u8]) -> Result<UserSkill> {
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

    write_skill(root, &fm.name, rebased)
}

fn write_skill(root: &Path, name: &str, files: Vec<(String, Vec<u8>)>) -> Result<UserSkill> {
    if !files.iter().any(|(rel, _)| rel == "SKILL.md") {
        return Err(anyhow!("skill folder must contain a SKILL.md at its root"));
    }
    let dir = store_dir(root).join(name);
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
    read_uploaded_at(&dir)
}

// --- mirrored coding-agent skills ---------------------------------------------

/// A skill folder belonging to an installed coding agent — its own, or one from
/// a plugin it has installed — mirrored into every session as-is.
struct Mirrored {
    /// Display name of the agent or plugin it came from, for the UI badge.
    origin: String,
    /// The session skills dir whose agent already loads this skill under this
    /// same bare name, so a session it hosts isn't handed a second copy.
    session_skills_dir: Option<&'static str>,
    dir: PathBuf,
    name: String,
    description: String,
}

/// Every mirrorable skill across the harnesses on this machine: each one's own
/// global skills dir, plus the skills that come with its installed plugins.
/// Sorted by name (then origin) so the first of a duplicated name always wins.
fn mirrored() -> Vec<Mirrored> {
    let mut out = Vec::new();
    for harness in registry() {
        if let Some(dir) = harness.global_skills_dir() {
            collect_mirrored(&dir, harness.name(), harness.session_skills_dir(), &mut out);
        }
        // A plugin's skills are registered namespaced (`runpod:flash`), never
        // bare, so the `/name` the composer offers resolves only from a copy —
        // including in a session the plugin's own agent hosts.
        for (plugin, dir) in harness.plugin_skills_dirs() {
            collect_mirrored(&dir, &plugin, None, &mut out);
        }
    }
    out.sort_by(|a, b| {
        (a.name.as_str(), a.origin.as_str()).cmp(&(b.name.as_str(), b.origin.as_str()))
    });
    out
}

/// Read one skills dir — a folder per skill, each with a `SKILL.md` — skipping
/// the `orx` shim, the built-in namespace, and anything malformed or oddly
/// named. A dir that isn't there yet is simply no skills.
fn collect_mirrored(
    dir: &Path,
    origin: &str,
    session_skills_dir: Option<&'static str>,
    out: &mut Vec<Mirrored>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
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
        // The frontmatter name is the `/name` and the dir we write into a
        // session, so skip anything that isn't a clean, free slug.
        if !is_valid_slug(&fm.name) || is_reserved(&fm.name) {
            continue;
        }
        out.push(Mirrored {
            origin: origin.to_string(),
            session_skills_dir,
            dir: path,
            name: fm.name,
            description: fm.description,
        });
    }
}

// --- list / find / delete -----------------------------------------------------

/// Read an uploaded skill folder. Its **directory name** is the id, not the
/// frontmatter `name` that created it: everything else — `/name` invocation,
/// delete, the copy into a session — resolves through the directory, and a
/// hand-edited `name:` must not detach the two.
fn read_uploaded_at(dir: &Path) -> Result<UserSkill> {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| is_valid_slug(n))
        .ok_or_else(|| anyhow!("`{}` is not a skill folder", dir.display()))?;
    let md_path = dir.join("SKILL.md");
    let content = fs::read_to_string(&md_path)
        .map_err(|e| anyhow!("could not read {}: {e}", md_path.display()))?;
    let fm = parse_frontmatter(&content)?;
    Ok(UserSkill {
        name,
        description: fm.description,
        origin: None,
        bytes: dir_size(dir),
        updated_at: mtime_ms(&md_path),
    })
}

/// Uploaded skills, name-sorted. Unreadable/malformed folders are skipped rather
/// than failing the whole listing.
fn list_uploaded_in(root: &Path) -> Vec<UserSkill> {
    let Ok(entries) = fs::read_dir(store_dir(root)) else {
        return Vec::new();
    };
    let mut out: Vec<UserSkill> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| read_uploaded_at(&e.path()).ok())
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Every skill the agent gets: the user's uploads plus whatever the installed
/// coding agents and their plugins have, name-sorted. One entry per `/name` — an
/// upload shadows a mirrored skill of the same name.
pub fn list() -> Vec<UserSkill> {
    list_in(&root(), &mirrored())
}

fn list_in(root: &Path, mirrored: &[Mirrored]) -> Vec<UserSkill> {
    let mut out = list_uploaded_in(root);
    for m in mirrored {
        if out.iter().any(|s| s.name == m.name) {
            continue;
        }
        // Same budget the session write applies, so the tab can't offer a `/name`
        // that never reaches the worktree.
        let Some(tally) = within_budget(&m.dir) else {
            continue;
        };
        out.push(UserSkill {
            name: m.name.clone(),
            description: m.description.clone(),
            origin: Some(m.origin.clone()),
            bytes: tally.bytes,
            updated_at: mtime_ms(&m.dir.join("SKILL.md")),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The folder each `/name` resolves to, uploads shadowing mirrored skills — the
/// same one-per-name resolution [`list_in`] shows.
///
/// `skills_dir_rel` is the session dir the folders are headed for, when they are
/// headed for one. A skill whose winning source is the agent hosting that
/// session is dropped *after* it has claimed the name, so a same-named skill
/// from a second agent can't quietly take its place in the worktree while the
/// dashboard shows the first.
fn source_dirs(
    root: &Path,
    mirrored: &[Mirrored],
    skills_dir_rel: Option<&str>,
) -> Vec<(String, PathBuf)> {
    // Through `list_uploaded_in`, so a folder the listing drops as unreadable
    // can't still win the name here and shadow a mirrored skill that works.
    let mut out: Vec<(String, PathBuf, Option<&'static str>)> = list_uploaded_in(root)
        .into_iter()
        .map(|skill| {
            let dir = store_dir(root).join(&skill.name);
            (skill.name, dir, None)
        })
        .collect();
    for m in mirrored {
        if !out.iter().any(|(name, ..)| *name == m.name) {
            out.push((m.name.clone(), m.dir.clone(), m.session_skills_dir));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.into_iter()
        .filter(|(_, _, host)| !matches!((host, skills_dir_rel), (Some(h), Some(rel)) if *h == rel))
        .map(|(name, dir, _)| (name, dir))
        .collect()
}

/// The Markdown body of a skill's `SKILL.md`, for the composer hover preview.
pub fn content(name: &str) -> Option<String> {
    content_in(&root(), &mirrored(), name)
}

fn content_in(root: &Path, mirrored: &[Mirrored], name: &str) -> Option<String> {
    let (_, dir) = source_dirs(root, mirrored, None)
        .into_iter()
        .find(|(n, _)| n == name)?;
    let content = fs::read_to_string(dir.join("SKILL.md")).ok()?;
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

/// Delete an uploaded skill. Mirrored ones are managed in the agent that owns
/// them, so they are not deletable here.
pub fn delete(name: &str) -> Result<()> {
    delete_in(&root(), name)
}

fn delete_in(root: &Path, name: &str) -> Result<()> {
    if !is_valid_slug(name) {
        return Err(anyhow!("invalid skill name"));
    }
    let dir = store_dir(root).join(name);
    if !dir.exists() {
        return Err(anyhow!("skill `{name}` not found"));
    }
    fs::remove_dir_all(&dir).map_err(|e| anyhow!("could not delete skill: {e}"))
}

// --- session wiring -----------------------------------------------------------

/// A manifest of the skill dirs [`write_into_session`] wrote last turn. It is
/// the record of what we own: only a dir named here may be replaced or pruned,
/// which keeps us off the built-in `orx-*` skills and off any `.claude/skills`
/// the project itself commits. It lives inside the (git-excluded) session skills
/// dir as a dotfile, so the harness never treats it as a skill — and the agent
/// can write to it, so every name read back out is re-validated.
const MANAGED_MANIFEST: &str = ".orx-user-skills";

/// Copy every applicable skill folder into the session worktree's native skills
/// dir, beside the built-in `orx-*` skills. Called fresh each turn from
/// `ensure_playbook`: a folder whose source changed is replaced, one that no
/// longer applies is pruned, and one we don't own is left alone — so a session's
/// skills track their sources with no drift and no collateral damage.
pub fn write_into_session(worktree: &Path, skills_dir_rel: &str) -> Result<()> {
    write_into_session_in(&root(), &mirrored(), worktree, skills_dir_rel)
}

fn write_into_session_in(
    root: &Path,
    mirrored: &[Mirrored],
    worktree: &Path,
    skills_dir_rel: &str,
) -> Result<()> {
    let base = worktree.join(skills_dir_rel);
    let previous = previously_managed(&base);
    let mut managed: Vec<String> = Vec::new();
    for (name, src) in source_dirs(root, mirrored, Some(skills_dir_rel)) {
        let Some(src_tally) = within_budget(&src) else {
            continue;
        };
        let dest = base.join(&name);
        let current = dest_matches_source(&src, src_tally, &dest);
        // A destination we didn't write is the project's own committed skill —
        // it wins, untouched. One that already matches is ours whatever the
        // manifest says, which is how a lost manifest heals itself.
        if dest.exists() && !current && !previous.contains(&name) {
            continue;
        }
        if !current {
            if dest.exists() {
                let _ = fs::remove_dir_all(&dest);
            }
            // Replace, don't merge: a re-upload with fewer files, or an upload
            // shadowing a mirrored skill, must not keep stale siblings. A copy
            // that fails skips this turn rather than failing the session.
            if copy_dir_all(&src, &dest).is_err() {
                let _ = fs::remove_dir_all(&dest);
                continue;
            }
        }
        managed.push(name);
    }
    // Prune what we wrote before and no longer applies — never a path we didn't.
    for name in &previous {
        if !managed.contains(name) {
            let _ = fs::remove_dir_all(base.join(name));
        }
    }
    if base.exists() {
        let _ = fs::write(base.join(MANAGED_MANIFEST), managed.join("\n"));
    }
    Ok(())
}

/// The names we wrote last turn. The manifest sits in the agent-writable
/// worktree, so a name from it is untrusted: `join` on an absolute or `..` path
/// escapes `base`, and these names are handed to `remove_dir_all`.
fn previously_managed(base: &Path) -> Vec<String> {
    fs::read_to_string(base.join(MANAGED_MANIFEST))
        .map(|manifest| {
            manifest
                .lines()
                .filter(|name| is_valid_slug(name))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `dest` already holds this exact skill, so the turn can skip a full
/// re-copy. Timestamps are no witness — macOS's `fs::copy` carries the source's
/// mtime across and Linux's doesn't — so the comparison is the source's
/// [`Tally`] plus a byte-identical `SKILL.md`, the one file that always changes
/// when someone edits a skill.
fn dest_matches_source(src: &Path, src_tally: Tally, dest: &Path) -> bool {
    if !dest.is_dir() || tally(dest, u64::MAX, u64::MAX) != Some(src_tally) {
        return false;
    }
    match (
        fs::read(src.join("SKILL.md")),
        fs::read(dest.join("SKILL.md")),
    ) {
        (Ok(from), Ok(to)) => from == to,
        _ => false,
    }
}

/// The tally of a folder we're about to mirror, or `None` when it blows the same
/// budget an upload has to fit — not something to copy into every session, every
/// turn.
fn within_budget(dir: &Path) -> Option<Tally> {
    tally(dir, MAX_FILES as u64, MAX_TOTAL_BYTES)
}

/// Build the instruction for one selected user skill. Its complete `SKILL.md`
/// is already in the worktree; the chat layer supplies the shared user request.
pub fn instructions(name: &str) -> Option<String> {
    instructions_in(&root(), &mirrored(), name)
}

fn instructions_in(root: &Path, mirrored: &[Mirrored], name: &str) -> Option<String> {
    source_dirs(root, mirrored, None)
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(name, _)| format!("Use the `{name}` skill."))
}

// --- fs helpers ---------------------------------------------------------------

/// Copy a skill folder. Symlinks are skipped rather than followed: a mirrored
/// folder is often a symlinked checkout, and a link pointing back up its own
/// tree would never terminate.
pub(crate) fn copy_dir_all(src: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest).map_err(|e| anyhow!("could not create {}: {e}", dest.display()))?;
    for entry in fs::read_dir(src).map_err(|e| anyhow!("could not read {}: {e}", src.display()))? {
        let entry = entry.map_err(|e| anyhow!("could not read dir entry: {e}"))?;
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if kind.is_dir() {
            copy_dir_all(&from, &to)?;
        } else {
            fs::copy(&from, &to).map_err(|e| anyhow!("could not copy {}: {e}", from.display()))?;
        }
    }
    Ok(())
}

/// What a folder holds, cheaply enough to compute every turn: how many files,
/// how many bytes, and a digest folded over every (relative path, size) pair so
/// that a rename or a move inside the folder shows up even when the totals
/// don't. Summed, not sequenced, because `read_dir` order isn't stable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Tally {
    files: u64,
    pub(crate) bytes: u64,
    digest: u64,
}

/// Walk `dir`, giving up (`None`) as soon as either cap is passed. Iterative and
/// symlink-skipping for the same reason as [`copy_dir_all`] — this walks
/// directories orx doesn't own.
pub(crate) fn tally(dir: &Path, max_files: u64, max_bytes: u64) -> Option<Tally> {
    use std::hash::{Hash, Hasher};

    let mut out = Tally::default();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        let Ok(entries) = fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            let path = entry.path();
            if kind.is_dir() {
                pending.push(path);
                continue;
            }
            let len = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
            out.files += 1;
            out.bytes += len;
            if out.files > max_files || out.bytes > max_bytes {
                return None;
            }
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            path.strip_prefix(dir).unwrap_or(&path).hash(&mut hasher);
            len.hash(&mut hasher);
            out.digest = out.digest.wrapping_add(hasher.finish());
        }
    }
    Some(out)
}

pub(crate) fn dir_size(dir: &Path) -> u64 {
    tally(dir, u64::MAX, u64::MAX)
        .map(|tally| tally.bytes)
        .unwrap_or(0)
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

    /// A skill folder standing in for one installed in a coding agent.
    fn mirrored_skill(origin: &str, dir: &Path, md: &str, extra: &[(&str, &[u8])]) -> Mirrored {
        mirrored_from(origin, None, dir, md, extra)
    }

    fn mirrored_from(
        origin: &str,
        session_skills_dir: Option<&'static str>,
        dir: &Path,
        md: &str,
        extra: &[(&str, &[u8])],
    ) -> Mirrored {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("SKILL.md"), md).unwrap();
        for (rel, body) in extra {
            fs::write(dir.join(rel), body).unwrap();
        }
        let fm = parse_frontmatter(md).unwrap();
        Mirrored {
            origin: origin.to_string(),
            session_skills_dir,
            dir: dir.to_path_buf(),
            name: fm.name,
            description: fm.description,
        }
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
    fn frontmatter_reads_multi_line_values() {
        // A quoted description wrapped over several indented lines — the shape
        // that used to drop the skill entirely.
        let fm = parse_frontmatter(concat!(
            "---\n",
            "name: math-olympiad\n",
            "description:\n",
            "  \"Solve competition math problems with adversarial\n",
            "  verification. Use when asked to 'prove this'.\"\n",
            "---\nbody",
        ))
        .unwrap();
        assert_eq!(fm.name, "math-olympiad");
        assert_eq!(
            fm.description,
            "Solve competition math problems with adversarial verification. Use when asked to 'prove this'."
        );

        // A folded block scalar.
        let fm =
            parse_frontmatter("---\nname: folded\ndescription: >-\n  one\n  two\n---\nb").unwrap();
        assert_eq!(fm.description, "one two");

        // A literal block keeps its line breaks.
        let fm =
            parse_frontmatter("---\nname: literal\ndescription: |\n  one\n  two\n---\nb").unwrap();
        assert_eq!(fm.description, "one\ntwo");

        // A comment ends the value above it instead of joining it.
        let fm = parse_frontmatter(concat!(
            "---\n",
            "name: commented\n",
            "# keep the description short\n",
            "description: Short. Use when testing.\n",
            "---\nbody",
        ))
        .unwrap();
        assert_eq!(fm.name, "commented");
        assert_eq!(fm.description, "Short. Use when testing.");

        // A value that merely starts with `>` is text, not a folded block.
        let fm = parse_frontmatter("---\nname: gt\ndescription: >50 tests run.\n---\nb").unwrap();
        assert_eq!(fm.description, ">50 tests run.");

        // A double-quoted scalar JSON rejects is unwrapped, not dropped.
        let fm = parse_frontmatter("---\nname: esc\ndescription: \"a \\q b\"\n---\nb").unwrap();
        assert_eq!(fm.description, "a \\q b");

        // A nested mapping's own keys are not mistaken for the top-level ones,
        // and don't swallow the value above them.
        let fm = parse_frontmatter(concat!(
            "---\n",
            "name: nested\n",
            "description: real one\n",
            "metadata:\n",
            "  name: not-the-skill\n",
            "---\nbody",
        ))
        .unwrap();
        assert_eq!(fm.name, "nested");
        assert_eq!(fm.description, "real one");
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
        let saved = save_skill_md_in(&root, skill_md("alpha").as_bytes()).unwrap();
        assert_eq!(saved.name, "alpha");
        assert_eq!(saved.origin, None);
        assert!(root.join("global/alpha/SKILL.md").exists());

        let list = list_in(&root, &[]);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "alpha");

        delete_in(&root, "alpha").unwrap();
        assert!(list_in(&root, &[]).is_empty());
        // Deleting a missing skill is an error.
        assert!(delete_in(&root, "alpha").is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_reserved_and_bad_names() {
        let root = temp_root();
        assert!(save_skill_md_in(&root, skill_md("orx-compute").as_bytes()).is_err());
        assert!(save_skill_md_in(&root, skill_md("lit-review").as_bytes()).is_err());
        let bad = "---\nname: Bad_Name\ndescription: nope. Use.\n---\nbody\n";
        assert!(save_skill_md_in(&root, bad.as_bytes()).is_err());
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
        let saved = save_zip_in(&root, &zip).unwrap();
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
        assert!(save_zip_in(&root, &zip).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn re_upload_replaces_prior_files() {
        let root = temp_root();
        let zip = make_zip(&[
            ("s/SKILL.md", skill_md("s").as_bytes()),
            ("s/old.txt", b"stale"),
        ]);
        save_zip_in(&root, &zip).unwrap();
        assert!(root.join("global/s/old.txt").exists());
        // Re-upload without old.txt must drop it.
        save_skill_md_in(&root, skill_md("s").as_bytes()).unwrap();
        assert!(!root.join("global/s/old.txt").exists());
        assert!(root.join("global/s/SKILL.md").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn mirrored_skills_are_listed_and_shadowed_by_uploads() {
        let root = temp_root();
        let agent_dir = temp_root();
        let mirrored = vec![
            mirrored_skill(
                "Claude Code",
                &agent_dir.join("dup"),
                &skill_md_desc("dup", "MIRRORED body"),
                &[],
            ),
            mirrored_skill(
                "Claude Code",
                &agent_dir.join("solo"),
                &skill_md("solo"),
                &[],
            ),
        ];
        save_skill_md_in(&root, skill_md_desc("dup", "UPLOADED body").as_bytes()).unwrap();

        let list = list_in(&root, &mirrored);
        assert_eq!(
            list.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["dup", "solo"]
        );
        let dup = &list[0];
        assert!(dup.description.contains("UPLOADED"), "upload must shadow");
        assert_eq!(dup.origin, None);
        assert_eq!(list[1].origin.as_deref(), Some("Claude Code"));

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&agent_dir);
    }

    #[test]
    fn write_into_session_copies_uploads_and_mirrored() {
        let root = temp_root();
        let agent_dir = temp_root();
        let wt = temp_root();
        // The mirrored `dup` carries an extra file the upload lacks.
        let mirrored = vec![
            mirrored_skill(
                "Codex",
                &agent_dir.join("dup"),
                &skill_md_desc("dup", "MIRRORED"),
                &[("extra.txt", b"mirrored-only")],
            ),
            mirrored_skill("Codex", &agent_dir.join("solo"), &skill_md("solo"), &[]),
        ];
        save_skill_md_in(&root, skill_md_desc("dup", "UPLOADED").as_bytes()).unwrap();

        write_into_session_in(&root, &mirrored, &wt, ".claude/skills").unwrap();
        assert!(wt.join(".claude/skills/solo/SKILL.md").exists());
        let dup = fs::read_to_string(wt.join(".claude/skills/dup/SKILL.md")).unwrap();
        assert!(dup.contains("UPLOADED"), "upload must shadow the mirror");
        assert!(
            !wt.join(".claude/skills/dup/extra.txt").exists(),
            "a shadowing upload must not keep the mirrored skill's files"
        );

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&agent_dir);
        let _ = fs::remove_dir_all(&wt);
    }

    #[test]
    fn a_session_is_not_handed_back_its_own_agents_skills() {
        let root = temp_root();
        let agent_dir = temp_root();
        let wt = temp_root();
        let mirrored = vec![
            mirrored_from(
                "Claude Code",
                Some(".claude/skills"),
                &agent_dir.join("native"),
                &skill_md("native"),
                &[],
            ),
            mirrored_from(
                "Codex",
                Some(".agents/skills"),
                &agent_dir.join("foreign"),
                &skill_md("foreign"),
                &[],
            ),
        ];

        write_into_session_in(&root, &mirrored, &wt, ".claude/skills").unwrap();
        assert!(
            !wt.join(".claude/skills/native").exists(),
            "Claude Code already loads its own skills from the user's home dir"
        );
        assert!(wt.join(".claude/skills/foreign/SKILL.md").exists());
        // Both are still listed and invocable — one just needs no copy.
        assert_eq!(list_in(&root, &mirrored).len(), 2);
        assert!(instructions_in(&root, &mirrored, "native").is_some());
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&agent_dir);
        let _ = fs::remove_dir_all(&wt);
    }

    #[test]
    fn instructions_invoke_user_skill() {
        let root = temp_root();
        save_skill_md_in(&root, skill_md("greeter").as_bytes()).unwrap();
        assert_eq!(
            instructions_in(&root, &[], "greeter").unwrap(),
            "Use the `greeter` skill."
        );
        assert_eq!(
            content_in(&root, &[], "greeter").unwrap(),
            "# greeter\nbody\n"
        );
        assert!(content_in(&root, &[], "../../etc/passwd").is_none());
        assert!(instructions_in(&root, &[], "unknown").is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn write_into_session_prunes_skills_that_no_longer_apply() {
        let root = temp_root();
        let agent_dir = temp_root();
        let wt = temp_root();
        let rel = ".claude/skills";
        let mirrored = vec![mirrored_skill(
            "Claude Code",
            &agent_dir.join("mirrored"),
            &skill_md("mirrored"),
            &[],
        )];
        save_skill_md_in(&root, skill_md("keep").as_bytes()).unwrap();
        save_skill_md_in(&root, skill_md("gone").as_bytes()).unwrap();
        write_into_session_in(&root, &mirrored, &wt, rel).unwrap();
        assert!(wt.join(rel).join("keep/SKILL.md").exists());
        assert!(wt.join(rel).join("gone/SKILL.md").exists());
        assert!(wt.join(rel).join("mirrored/SKILL.md").exists());

        // Delete one skill and uninstall the mirrored one, re-run: both must go.
        delete_in(&root, "gone").unwrap();
        write_into_session_in(&root, &[], &wt, rel).unwrap();
        assert!(wt.join(rel).join("keep/SKILL.md").exists());
        assert!(
            !wt.join(rel).join("gone").exists(),
            "deleted skill must be pruned"
        );
        assert!(
            !wt.join(rel).join("mirrored").exists(),
            "a skill removed from its coding agent must be pruned"
        );
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&agent_dir);
        let _ = fs::remove_dir_all(&wt);
    }

    #[test]
    fn project_scoped_skills_migrate_into_the_store() {
        let root = temp_root();
        save_skill_md_in(&root, skill_md_desc("dup", "GLOBAL").as_bytes()).unwrap();
        for (project, name) in [("p1", "dup"), ("p1", "moved"), ("p2", "other")] {
            let dir = root.join("projects").join(project).join(name);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("SKILL.md"), skill_md_desc(name, "PROJECT")).unwrap();
        }

        migrate_project_scoped(&root);
        let names: Vec<String> = list_in(&root, &[]).into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["dup", "moved", "other"]);
        // The global of a colliding name keeps the `/name`...
        let dup = fs::read_to_string(root.join("global/dup/SKILL.md")).unwrap();
        assert!(dup.contains("GLOBAL"));
        // ...and the project copy it shadowed is left on disk rather than
        // deleted, since this is the user's only copy of it.
        let kept = fs::read_to_string(root.join("projects/p1/dup/SKILL.md")).unwrap();
        assert!(kept.contains("PROJECT"));
        assert!(
            !root.join("projects/p2").exists(),
            "an emptied project goes"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn collect_mirrored_takes_only_usable_skill_folders() {
        let agent_dir = temp_root();
        for (dir, md) in [
            ("good", skill_md("good")),
            ("orx-git", skill_md("orx-git")),
            ("shim", skill_md("plan")),
            ("shouty", skill_md_desc("Bad_Name", "Nope. Use never.")),
            ("builtin", skill_md("lit-review")),
        ] {
            let path = agent_dir.join(dir);
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("SKILL.md"), md).unwrap();
        }
        // A plain directory with no SKILL.md, and a stray file.
        fs::create_dir_all(agent_dir.join("notes")).unwrap();
        fs::write(agent_dir.join("README.md"), b"hi").unwrap();

        let mut out = Vec::new();
        collect_mirrored(&agent_dir, "Claude Code", None, &mut out);
        assert_eq!(
            out.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
            ["good"]
        );
        let _ = fs::remove_dir_all(&agent_dir);
    }

    #[test]
    fn a_session_keeps_a_skill_the_repo_ships_itself() {
        let root = temp_root();
        let wt = temp_root();
        let rel = ".claude/skills";
        // The project commits its own `code-review` skill into the worktree.
        let committed = wt.join(rel).join("code-review");
        fs::create_dir_all(&committed).unwrap();
        fs::write(committed.join("SKILL.md"), "REPO COPY").unwrap();
        save_skill_md_in(&root, skill_md("code-review").as_bytes()).unwrap();

        write_into_session_in(&root, &[], &wt, rel).unwrap();
        assert_eq!(
            fs::read_to_string(committed.join("SKILL.md")).unwrap(),
            "REPO COPY",
            "a skill dir we never wrote is not ours to replace"
        );
        // ...and it is never pruned either, since it never enters the manifest.
        write_into_session_in(&root, &[], &wt, rel).unwrap();
        assert!(committed.join("SKILL.md").exists());
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&wt);
    }

    #[test]
    fn an_unchanged_skill_is_not_recopied_into_the_session() {
        use std::os::unix::fs::MetadataExt;

        let root = temp_root();
        let wt = temp_root();
        let rel = ".claude/skills";
        save_skill_md_in(&root, skill_md("steady").as_bytes()).unwrap();

        write_into_session_in(&root, &[], &wt, rel).unwrap();
        let written = wt.join(rel).join("steady/SKILL.md");
        let first = fs::metadata(&written).unwrap().ino();

        // A second turn with an unchanged source must leave the file alone. The
        // inode is the witness: a re-copy removes the dir and writes a new file.
        write_into_session_in(&root, &[], &wt, rel).unwrap();
        assert_eq!(fs::metadata(&written).unwrap().ino(), first);

        // Editing the source does bring the copy forward.
        save_skill_md_in(
            &root,
            skill_md_desc("steady", "Now it says something else. Use when testing.").as_bytes(),
        )
        .unwrap();
        write_into_session_in(&root, &[], &wt, rel).unwrap();
        assert!(fs::read_to_string(&written)
            .unwrap()
            .contains("something else"));
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&wt);
    }

    #[test]
    fn a_folder_fingerprint_notices_edits_renames_and_extra_files() {
        let src = temp_root();
        let dest = temp_root();
        fs::create_dir_all(src.join("references")).unwrap();
        fs::write(src.join("SKILL.md"), skill_md("same")).unwrap();
        fs::write(src.join("references/palette.md"), b"blue").unwrap();
        copy_dir_all(&src, &dest).unwrap();
        let fingerprint = |dir: &Path| tally(dir, u64::MAX, u64::MAX).unwrap();
        assert!(dest_matches_source(&src, fingerprint(&src), &dest));

        // A rename keeps the file count and the byte total identical.
        fs::rename(
            src.join("references/palette.md"),
            src.join("references/colors.md"),
        )
        .unwrap();
        assert!(!dest_matches_source(&src, fingerprint(&src), &dest));

        // So does an edit to SKILL.md of exactly the same length.
        copy_dir_all(&src, &dest).unwrap();
        fs::write(src.join("SKILL.md"), skill_md("samf")).unwrap();
        assert!(!dest_matches_source(&src, fingerprint(&src), &dest));

        // A destination that isn't there at all never matches.
        assert!(!dest_matches_source(
            &src,
            fingerprint(&src),
            &src.join("nope")
        ));
        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn a_mirrored_folder_over_the_upload_budget_is_neither_listed_nor_copied() {
        let root = temp_root();
        let agent_dir = temp_root();
        let wt = temp_root();
        let mirrored = vec![mirrored_skill(
            "Claude Code",
            &agent_dir.join("huge"),
            &skill_md("huge"),
            &[],
        )];
        fs::write(
            agent_dir.join("huge/corpus.bin"),
            vec![0u8; (MAX_TOTAL_BYTES + 1) as usize],
        )
        .unwrap();

        assert!(list_in(&root, &mirrored).is_empty());
        write_into_session_in(&root, &mirrored, &wt, ".claude/skills").unwrap();
        assert!(!wt.join(".claude/skills/huge").exists());
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&agent_dir);
        let _ = fs::remove_dir_all(&wt);
    }

    #[test]
    fn a_long_description_is_kept_and_truncated_not_dropped() {
        let long = "Sprawls on. ".repeat(400);
        let fm = parse_frontmatter(&skill_md_desc("verbose", &long)).unwrap();
        assert_eq!(fm.name, "verbose");
        assert_eq!(fm.description.chars().count(), MAX_DESCRIPTION_LEN);
        assert!(fm.description.starts_with("Sprawls on."));
    }

    #[test]
    fn an_unreadable_upload_does_not_shadow_a_working_mirrored_skill() {
        let root = temp_root();
        let agent_dir = temp_root();
        let wt = temp_root();
        let mirrored = vec![mirrored_skill(
            "runpod",
            &agent_dir.join("flash"),
            &skill_md_desc("flash", "MIRRORED. Use when testing."),
            &[],
        )];
        // An upload interrupted mid-write leaves a folder with no usable
        // SKILL.md; the listing skips it, so the session must skip it too.
        fs::create_dir_all(store_dir(&root).join("flash")).unwrap();
        fs::write(store_dir(&root).join("flash/SKILL.md"), b"not frontmatter").unwrap();

        let listed = list_in(&root, &mirrored);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].origin.as_deref(), Some("runpod"));

        write_into_session_in(&root, &mirrored, &wt, ".agents/skills").unwrap();
        let written = fs::read_to_string(wt.join(".agents/skills/flash/SKILL.md")).unwrap();
        assert!(
            written.contains("MIRRORED"),
            "the worktree must run what the dashboard shows"
        );
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&agent_dir);
        let _ = fs::remove_dir_all(&wt);
    }

    #[test]
    fn a_crafted_manifest_cannot_delete_outside_the_skills_dir() {
        let root = temp_root();
        let wt = temp_root();
        let rel = ".claude/skills";
        let outsider = wt.join("keep-me");
        fs::create_dir_all(&outsider).unwrap();
        fs::create_dir_all(wt.join(rel)).unwrap();
        // The agent can write in its own worktree, so the manifest is untrusted.
        fs::write(
            wt.join(rel).join(MANAGED_MANIFEST),
            format!("../keep-me\n{}\n", outsider.display()),
        )
        .unwrap();

        write_into_session_in(&root, &[], &wt, rel).unwrap();
        assert!(
            outsider.is_dir(),
            "a `..` name must never reach remove_dir_all"
        );
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&wt);
    }

    #[test]
    fn an_uploaded_skill_is_keyed_by_its_folder_not_its_frontmatter() {
        let root = temp_root();
        save_skill_md_in(&root, skill_md("alpha").as_bytes()).unwrap();
        // Hand-edit the stored SKILL.md to claim a different name.
        fs::write(
            root.join("global/alpha/SKILL.md"),
            skill_md_desc("beta", "Renamed in place. Use when testing."),
        )
        .unwrap();

        let listed = list_in(&root, &[]);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "alpha", "the folder is the id");
        assert_eq!(
            source_dirs(&root, &[], None),
            vec![("alpha".to_string(), root.join("global/alpha"))]
        );
        assert!(instructions_in(&root, &[], "alpha").is_some());
        assert!(delete_in(&root, "alpha").is_ok());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_hosting_agents_skill_is_dropped_only_after_it_wins_its_name() {
        let root = temp_root();
        let agent_dir = temp_root();
        let wt = temp_root();
        // Both agents ship a `pdf`; Claude Code sorts first, so it owns the name.
        let mirrored = vec![
            mirrored_from(
                "Claude Code",
                Some(".claude/skills"),
                &agent_dir.join("claude-pdf"),
                &skill_md_desc("pdf", "CLAUDE copy. Use when testing."),
                &[],
            ),
            mirrored_from(
                "Codex",
                Some(".agents/skills"),
                &agent_dir.join("codex-pdf"),
                &skill_md_desc("pdf", "CODEX copy. Use when testing."),
                &[],
            ),
        ];

        write_into_session_in(&root, &mirrored, &wt, ".claude/skills").unwrap();
        assert!(
            !wt.join(".claude/skills/pdf").exists(),
            "the session must not run Codex's `pdf` while the dashboard shows Claude Code's"
        );
        assert_eq!(list_in(&root, &mirrored).len(), 1);
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&agent_dir);
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
        assert!(save_zip_in(&root, &big).is_err());

        let skill = skill_md("s");
        let many: Vec<(String, Vec<u8>)> = (0..MAX_FILES + 1)
            .map(|i| (format!("s/f{i}.txt"), b"x".to_vec()))
            .collect();
        let mut entries: Vec<(&str, &[u8])> = vec![("s/SKILL.md", skill.as_bytes())];
        for (n, b) in &many {
            entries.push((n.as_str(), b.as_slice()));
        }
        assert!(save_zip_in(&root, &make_zip(&entries)).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn quoted_frontmatter_name_is_accepted() {
        let root = temp_root();
        let md =
            "---\nname: \"data-cleaner\"\ndescription: Clean CSVs. Use when asked.\n---\nbody\n";
        let saved = save_skill_md_in(&root, md.as_bytes()).unwrap();
        assert_eq!(saved.name, "data-cleaner");
        let _ = fs::remove_dir_all(&root);
    }
}
