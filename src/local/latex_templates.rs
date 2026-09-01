//! User-uploaded LaTeX templates — a conference class, a lab preprint style, or
//! a bare preamble the agent should follow instead of the built-in one.
//!
//! Stored beside the user's skills, in one place — `data_dir()/latex-templates/global/<name>/`
//! — and available to every project.
//!
//! A template is a folder: one `.tex` entry point plus whatever `.cls`, `.sty`,
//! `.bst`, or `.bib` files it needs. Every template is copied into
//! the session worktree under [`SESSION_DIR_REL`] each turn, which is what lets
//! the agent read one *and* lets the compiler find its class files once the
//! agent copies them next to the paper (see the `orx-paper` skill).

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{anyhow, Result};
use crate::local::user_skills::{
    basename, copy_dir_all, depth, dir_size, migrate_project_scoped, mtime_ms, store_dir, tally,
};

/// Where templates land inside a session worktree. Under `.orx/` because they
/// are inputs the agent copies from, not part of the paper itself.
pub const SESSION_DIR_REL: &str = ".orx/latex-templates";

const MANAGED_MANIFEST: &str = ".orx-latex-templates";

/// A conference bundle is a handful of style files, not a tarball.
const MAX_FILES: usize = 200;
const MAX_TOTAL_BYTES: u64 = 20 * 1024 * 1024;
const MAX_NAME_LEN: usize = 48;

/// One uploaded template, as served to the UI.
#[derive(Clone, Debug)]
pub struct LatexTemplate {
    pub name: String,
    /// Relative path of the `.tex` the agent should start from.
    pub entry: String,
    /// Every other file shipped with it — styles, logos, bibliographies.
    pub support_files: Vec<String>,
    pub bytes: u64,
    pub updated_at: i64,
}

/// Same shape as the skills store, down to the retired per-project layout it
/// folds in on the way past — see [`super::user_skills::migrate_project_scoped`].
fn root() -> PathBuf {
    let root = crate::store::data_dir().join("latex-templates");
    migrate_project_scoped(&root);
    root
}

/// `^[a-z0-9]+(-[a-z0-9]+)*$` from an arbitrary upload filename.
fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.truncate(MAX_NAME_LEN);
    out.trim_matches('-').to_string()
}

/// Template name from the uploaded filename, minus its extension.
fn name_from_filename(filename: &str) -> Result<String> {
    let base = basename(filename);
    let stem = base.rsplit_once('.').map(|(s, _)| s).unwrap_or(base);
    let name = slug(stem);
    if name.is_empty() {
        return Err(anyhow!(
            "could not derive a template name from `{filename}` — rename it to something like `neurips-2024.zip`"
        ));
    }
    Ok(name)
}

fn extension(path: &str) -> String {
    basename(path)
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Archive noise no build ever reads. Everything else is kept — a class file
/// can pull in a logo, a font, or a data file, and guessing wrong breaks it.
fn is_junk(path: &str) -> bool {
    path.starts_with("__MACOSX/") || basename(path) == ".DS_Store"
}

/// The `.tex` the agent should open first: one that declares a document class,
/// else the shallowest, breaking ties by name so the pick is deterministic.
fn pick_entry(files: &[(String, Vec<u8>)]) -> Option<String> {
    files
        .iter()
        .filter(|(name, _)| extension(name) == "tex")
        .min_by_key(|(name, body)| {
            let declares = !String::from_utf8_lossy(body).contains("\\documentclass");
            (declares, depth(name), name.clone())
        })
        .map(|(name, _)| name.clone())
}

/// Save an uploaded `.tex` or `.zip` as a template folder.
pub fn save_upload(filename: &str, bytes: &[u8]) -> Result<LatexTemplate> {
    save_upload_in(&root(), filename, bytes)
}

fn save_upload_in(root: &Path, filename: &str, bytes: &[u8]) -> Result<LatexTemplate> {
    let name = name_from_filename(filename)?;
    let files = match extension(filename).as_str() {
        "zip" => read_zip(bytes)?,
        "tex" => {
            if bytes.len() as u64 > MAX_TOTAL_BYTES {
                return Err(anyhow!("template is too large"));
            }
            std::str::from_utf8(bytes)
                .map_err(|_| anyhow!("a .tex template must be UTF-8 text"))?;
            vec![(format!("{name}.tex"), bytes.to_vec())]
        }
        other => {
            return Err(anyhow!(
                "unsupported template file `.{other}` — upload a .tex or a .zip"
            ))
        }
    };
    write_template(root, &name, files)
}

fn read_zip(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    use std::io::{Cursor, Read};

    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| anyhow!("not a valid .zip archive: {e}"))?;
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
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
        if is_junk(&name) {
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
        files.push((name, buf));
    }
    if files.is_empty() {
        return Err(anyhow!("the .zip is empty"));
    }
    // Strip only the directory every entry shares, so a bundle shaped like
    // `icml.sty` + `example/paper.tex` keeps its style files. Taking the entry's
    // own parent would drop everything outside it, and the upload would look
    // like it worked.
    let prefix = common_directory(&files);
    let mut rebased = Vec::new();
    for (name, buf) in files {
        // The prefix comes from these very paths, so this always strips.
        let rel = name.strip_prefix(&prefix).unwrap_or(&name);
        if rel.is_empty()
            || rel
                .split('/')
                .any(|seg| seg.is_empty() || seg == "." || seg == "..")
        {
            return Err(anyhow!("zip contains an unsafe path: {name}"));
        }
        rebased.push((rel.to_string(), buf));
    }
    Ok(rebased)
}

/// The directory prefix shared by every path, `""` when they share none.
fn common_directory(files: &[(String, Vec<u8>)]) -> String {
    let mut shared: Option<Vec<&str>> = None;
    for (name, _) in files {
        let dirs: Vec<&str> = name.split('/').rev().skip(1).collect::<Vec<_>>();
        let dirs: Vec<&str> = dirs.into_iter().rev().collect();
        shared = Some(match shared {
            None => dirs,
            Some(current) => current
                .iter()
                .zip(dirs.iter())
                .take_while(|(a, b)| a == b)
                .map(|(a, _)| *a)
                .collect(),
        });
    }
    match shared {
        Some(dirs) if !dirs.is_empty() => format!("{}/", dirs.join("/")),
        _ => String::new(),
    }
}

fn write_template(root: &Path, name: &str, files: Vec<(String, Vec<u8>)>) -> Result<LatexTemplate> {
    if pick_entry(&files).is_none() {
        return Err(anyhow!("a template must contain at least one .tex file"));
    }
    let dir = store_dir(root).join(name);
    // A re-upload fully replaces the prior version — no stale sibling files.
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|e| anyhow!("could not replace template: {e}"))?;
    }
    for (rel, buf) in &files {
        let dest = dir.join(rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| anyhow!("could not create {}: {e}", parent.display()))?;
        }
        fs::write(&dest, buf).map_err(|e| anyhow!("could not write {}: {e}", dest.display()))?;
    }
    read_template_at(&dir)
}

/// Read a stored template folder back into its serialized form.
fn read_template_at(dir: &Path) -> Result<LatexTemplate> {
    let name = dir
        .file_name()
        .ok_or_else(|| anyhow!("template has no name"))?
        .to_string_lossy()
        .into_owned();
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    collect(dir, dir, &mut files)?;
    let entry = pick_entry(&files).ok_or_else(|| anyhow!("template has no .tex"))?;
    let mut support_files: Vec<String> = files
        .iter()
        .map(|(rel, _)| rel.clone())
        .filter(|rel| *rel != entry)
        .collect();
    support_files.sort();
    Ok(LatexTemplate {
        updated_at: mtime_ms(&dir.join(&entry)),
        bytes: dir_size(dir),
        name,
        entry,
        support_files,
    })
}

/// Relative paths of every file under `dir`, with `.tex` contents attached.
fn collect(base: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<()> {
    for entry in fs::read_dir(dir).map_err(|e| anyhow!("could not read {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| anyhow!("could not read dir entry: {e}"))?;
        let path = entry.path();
        if path.is_dir() {
            collect(base, &path, out)?;
            continue;
        }
        let Ok(rel) = path.strip_prefix(base) else {
            continue;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        // Only `.tex` bodies are inspected (for `\documentclass`), and they are
        // small — read them whole so the entry pick matches the upload's.
        let body = if extension(&rel) == "tex" {
            fs::read(&path).unwrap_or_default()
        } else {
            Vec::new()
        };
        out.push((rel, body));
    }
    Ok(())
}

/// Every uploaded template, name-sorted. Unreadable folders are skipped rather
/// than failing the whole listing.
pub fn list() -> Vec<LatexTemplate> {
    list_in(&root())
}

fn list_in(root: &Path) -> Vec<LatexTemplate> {
    let Ok(entries) = fs::read_dir(store_dir(root)) else {
        return Vec::new();
    };
    let mut out: Vec<LatexTemplate> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| read_template_at(&e.path()).ok())
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub fn delete(name: &str) -> Result<()> {
    delete_in(&root(), name)
}

fn delete_in(root: &Path, name: &str) -> Result<()> {
    // `Path::join("")` returns the parent, so an empty name would delete the
    // whole store directory — every template the user has uploaded.
    if name.is_empty() || name != slug(name) {
        return Err(anyhow!("unknown template `{name}`"));
    }
    let dir = store_dir(root).join(name);
    if !dir.is_dir() {
        return Err(anyhow!("unknown template `{name}`"));
    }
    fs::remove_dir_all(&dir).map_err(|e| anyhow!("could not delete template: {e}"))?;
    Ok(())
}

/// Copy every template into the session worktree, replacing what was
/// there and pruning templates the user has since deleted — same freshness
/// contract as the skills dir.
pub fn write_into_session(worktree: &Path) -> Result<()> {
    write_into_session_in(&root(), worktree)
}

fn write_into_session_in(root: &Path, worktree: &Path) -> Result<()> {
    let base = worktree.join(SESSION_DIR_REL);
    let mut managed: Vec<String> = Vec::new();
    if let Ok(entries) = fs::read_dir(store_dir(root)) {
        for entry in entries.flatten() {
            let src = entry.path();
            if !src.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let dest = base.join(&name);
            let src_tally = tally(&src, u64::MAX, u64::MAX);
            // A bundle that hasn't changed is left alone; this runs every turn.
            if src_tally.is_none() || tally(&dest, u64::MAX, u64::MAX) != src_tally {
                if dest.exists() {
                    let _ = fs::remove_dir_all(&dest);
                }
                // One unreadable template folder skips its turn rather than
                // failing the session, the same as a skill that won't copy.
                if copy_dir_all(&src, &dest).is_err() {
                    let _ = fs::remove_dir_all(&dest);
                    continue;
                }
            }
            if !managed.contains(&name) {
                managed.push(name);
            }
        }
    }
    if let Ok(prev) = fs::read_to_string(base.join(MANAGED_MANIFEST)) {
        // The manifest sits in the agent-writable worktree, so a name from it is
        // untrusted: `join` on an absolute or `..` path escapes `base`.
        for name in prev.lines().filter(|n| !n.is_empty() && *n == slug(n)) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "orx-latex-templates-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("tmp");
        dir
    }

    fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            for (name, body) in files {
                w.start_file::<_, ()>(*name, zip::write::SimpleFileOptions::default())
                    .expect("entry");
                w.write_all(body).expect("write");
            }
            w.finish().expect("finish");
        }
        buf
    }

    #[test]
    fn a_single_tex_upload_becomes_a_named_template() {
        let root = tmp();
        let saved = save_upload_in(
            &root,
            "NeurIPS 2024 Preprint.tex",
            b"\\documentclass{article}\n",
        )
        .expect("save");
        assert_eq!(saved.name, "neurips-2024-preprint");
        assert_eq!(saved.entry, "neurips-2024-preprint.tex");
        assert!(saved.support_files.is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_zip_keeps_style_files_and_picks_the_document_class_tex() {
        let root = tmp();
        let bytes = zip_of(&[
            ("neurips_2024/neurips_2024.sty", b"% style"),
            (
                "neurips_2024/example_paper.tex",
                b"\\documentclass{article}\n",
            ),
            ("neurips_2024/refs.bib", b"@article{a,}"),
            // A class that draws a logo needs its image: dropping non-LaTeX
            // files used to fail the build fatally at image inclusion.
            ("neurips_2024/include/logo.png", b"\x89PNG"),
            ("__MACOSX/._x", b"junk"),
        ]);
        let saved = save_upload_in(&root, "neurips-2024.zip", &bytes).expect("save");
        assert_eq!(saved.name, "neurips-2024");
        // The wrapping directory is stripped, so the .sty sits beside the entry.
        assert_eq!(saved.entry, "example_paper.tex");
        assert_eq!(
            saved.support_files,
            vec!["include/logo.png", "neurips_2024.sty", "refs.bib"]
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_preamble_only_tex_wins_over_a_deeper_one_without_documentclass() {
        let root = tmp();
        let bytes = zip_of(&[
            ("t/sections/intro.tex", b"Some prose."),
            ("t/main.tex", b"\\documentclass{article}\n"),
        ]);
        let saved = save_upload_in(&root, "t.zip", &bytes).expect("save");
        assert_eq!(saved.entry, "main.tex");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn uploads_without_a_tex_are_rejected() {
        let root = tmp();
        let bytes = zip_of(&[("style/only.sty", b"% style")]);
        let err = save_upload_in(&root, "style.zip", &bytes).expect_err("must reject");
        assert!(err.to_string().contains(".tex"));
        assert!(save_upload_in(&root, "notes.md", b"# hi").is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_filename_with_no_usable_characters_is_refused() {
        let root = tmp();
        let err =
            save_upload_in(&root, "___.tex", b"\\documentclass{article}").expect_err("must refuse");
        assert!(err.to_string().contains("could not derive a template name"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_re_upload_replaces_the_template_of_the_same_name() {
        let root = tmp();
        save_upload_in(
            &root,
            "house.zip",
            &zip_of(&[
                ("house.tex", b"\\documentclass{article}"),
                ("old.sty", b"% stale"),
            ]),
        )
        .expect("first");
        let saved = save_upload_in(&root, "house.tex", b"\\documentclass{report}").expect("second");

        assert_eq!(list_in(&root).len(), 1, "one entry per name");
        assert!(saved.support_files.is_empty(), "stale siblings are dropped");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_session_gets_every_template_and_loses_deleted_ones() {
        let root = tmp();
        let worktree = tmp();
        save_upload_in(&root, "shared.tex", b"\\documentclass{article}").expect("shared");
        save_upload_in(&root, "mine.tex", b"\\documentclass{article}").expect("mine");

        write_into_session_in(&root, &worktree).expect("write");
        let base = worktree.join(SESSION_DIR_REL);
        assert!(base.join("shared/shared.tex").is_file());
        assert!(base.join("mine/mine.tex").is_file());

        delete_in(&root, "mine").expect("delete");
        write_into_session_in(&root, &worktree).expect("rewrite");
        assert!(base.join("shared/shared.tex").is_file());
        assert!(
            !base.join("mine").exists(),
            "a deleted template must not linger in the worktree"
        );
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&worktree);
    }

    #[test]
    fn project_scoped_templates_migrate_into_the_store() {
        let root = tmp();
        save_upload_in(&root, "house.tex", b"\\documentclass{article}% global").expect("global");
        for (project, name) in [("p1", "house"), ("p1", "moved")] {
            let dir = root.join("projects").join(project).join(name);
            fs::create_dir_all(&dir).expect("mkdir");
            fs::write(
                dir.join(format!("{name}.tex")),
                b"\\documentclass{report}% project",
            )
            .expect("write");
        }

        migrate_project_scoped(&root);
        let names: Vec<String> = list_in(&root).into_iter().map(|t| t.name).collect();
        assert_eq!(names, ["house", "moved"]);
        // The global of a colliding name keeps the name, and the project copy it
        // shadowed stays on disk rather than being deleted.
        let house = fs::read_to_string(root.join("global/house/house.tex")).expect("read");
        assert!(house.contains("global"));
        assert!(root.join("projects/p1/house/house.tex").is_file());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_zip_cannot_write_outside_its_template_folder() {
        let root = tmp();
        let bytes = zip_of(&[
            ("../escape.tex", b"\\documentclass{article}"),
            ("ok/main.tex", b"\\documentclass{article}"),
        ]);
        // A traversal entry fails the whole archive rather than being skipped —
        // an archive that tried is not one to extract the rest of.
        let err = save_upload_in(&root, "t.zip", &bytes).expect_err("must reject the archive");
        assert!(err.to_string().contains("unsafe path"));
        assert!(!root.parent().unwrap().join("escape.tex").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_bundle_keeps_style_files_that_sit_beside_the_entrys_directory() {
        let root = tmp();
        // The layout real conference bundles ship: styles at the root, the
        // example paper one level down.
        let bytes = zip_of(&[
            ("icml2024.sty", b"% style"),
            ("icml2024.bst", b"% bib style"),
            ("example/example_paper.tex", b"\\documentclass{article}\n"),
        ]);
        let saved = save_upload_in(&root, "icml.zip", &bytes).expect("save");
        assert_eq!(saved.entry, "example/example_paper.tex");
        assert_eq!(
            saved.support_files,
            vec!["icml2024.bst", "icml2024.sty"],
            "styles outside the entry's directory must survive"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_single_wrapping_directory_is_still_stripped() {
        let root = tmp();
        let bytes = zip_of(&[
            ("bundle/main.tex", b"\\documentclass{article}"),
            ("bundle/styles/x.sty", b"% style"),
        ]);
        let saved = save_upload_in(&root, "b.zip", &bytes).expect("save");
        assert_eq!(saved.entry, "main.tex");
        assert_eq!(saved.support_files, vec!["styles/x.sty"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_empty_name_cannot_delete_the_whole_store() {
        let root = tmp();
        save_upload_in(&root, "keep.tex", b"\\documentclass{article}").expect("save");
        // `Path::join("")` yields the parent, so this once wiped every template.
        assert!(delete_in(&root, "").is_err());
        assert_eq!(list_in(&root).len(), 1, "template survived");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn delete_rejects_a_crafted_name() {
        let root = tmp();
        assert!(delete_in(&root, "../../etc").is_err());
        assert!(delete_in(&root, "missing").is_err());
        let _ = fs::remove_dir_all(&root);
    }
}
