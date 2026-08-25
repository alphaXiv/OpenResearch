//! Compile a `.tex` file with whatever LaTeX toolchain the machine already has.
//!
//! Local-only, like `crate::editors`: `orx up` runs on the user's own machine,
//! so the API process can shell out to their TeX install. Nothing is bundled —
//! a machine with no toolchain gets a clear message instead of a silent
//! failure. The documents are agent-authored, so every engine runs
//! non-interactively and with shell escape off.
//!
//! The target is Overleaf parity, which means `latexmk` over a real TeX
//! distribution: it picks the engine the document asks for, runs biber or
//! bibtex when the bibliography needs it, and repeats passes until references
//! settle. Where latexmk is absent we drive the engine directly and orchestrate
//! that ourselves; where nothing but `tectonic` exists we use it and say so,
//! since it is XeTeX and cannot honour a document that wants LuaLaTeX.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::error::{anyhow, Result};
use crate::local::shell_env::{find_on_path, search_path};

/// The TeX engine a document is written for. Overleaf's default is pdfLaTeX and
/// so is ours; a document says otherwise with a `% !TeX program` line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Program {
    Pdf,
    Xe,
    Lua,
}

impl Program {
    /// The engine binary, for driving it without latexmk. Overleaf names its
    /// engines the same way, so this doubles as the `engine` it accepts.
    pub(crate) fn binary(self) -> &'static str {
        match self {
            Program::Pdf => "pdflatex",
            Program::Xe => "xelatex",
            Program::Lua => "lualatex",
        }
    }

    /// The latexmk flag that selects this engine.
    fn latexmk_flag(self) -> &'static str {
        match self {
            Program::Pdf => "-pdf",
            Program::Xe => "-xelatex",
            Program::Lua => "-lualatex",
        }
    }

    /// True when tectonic — which is XeTeX — can stand in for this engine.
    fn served_by_tectonic(self) -> bool {
        matches!(self, Program::Xe)
    }
}

/// How we drive the compile, in preference order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Driver {
    /// What Overleaf runs: engine selection, bibliography, repeat passes.
    Latexmk,
    /// The engine binary itself; we orchestrate bibliography and passes.
    Direct,
    /// Self-contained, fetches its own packages, but always XeTeX.
    Tectonic,
}

/// Only the first lines are scanned — a magic comment must precede the
/// document, and this keeps a stray match deep in the body from mattering.
const PROGRAM_COMMENT_LINES: usize = 16;

/// Read a `% !TeX program = lualatex` line. TeXShop writes `TS-program`, most
/// editors and Overleaf users write `program`; both appear in real papers, so
/// both are accepted, case-insensitively.
pub fn program_from_source(text: &str) -> Option<Program> {
    text.lines()
        .take(PROGRAM_COMMENT_LINES)
        .find_map(program_from_line)
}

/// One magic comment, or None if this line is not one. Kept separate so a line
/// that fails to match moves on to the next: real papers stack these, and
/// `% !TeX root` or `% !BIB program` above the engine line must not stop the
/// scan before it is reached.
fn program_from_line(line: &str) -> Option<Program> {
    let rest = line
        .trim_start_matches('\u{feff}')
        .trim_start()
        .strip_prefix('%')?;
    let rest = rest.trim_start_matches('%').trim_start();
    let rest = rest.strip_prefix('!')?.trim_start().to_ascii_lowercase();
    let rest = rest
        .strip_prefix("tex")
        .or_else(|| rest.strip_prefix("latex"))?
        .trim_start();
    let rest = rest.strip_prefix("ts-").unwrap_or(rest);
    let value = rest
        .strip_prefix("program")?
        .trim_start()
        .strip_prefix('=')?;
    match value.trim() {
        "pdflatex" | "pdflatexmk" => Some(Program::Pdf),
        "xelatex" | "xelatexmk" => Some(Program::Xe),
        "lualatex" | "lualatexmk" => Some(Program::Lua),
        _ => None,
    }
}

/// Per engine invocation. latexmk and tectonic run their own passes inside one
/// child, so for them this is the whole-document budget.
const PASS_TIMEOUT: Duration = Duration::from_secs(120);

/// Grace for the output readers once the process tree is gone. Reaching it
/// means a descendant still holds the pipe, so the request completes without
/// the transcript rather than hanging on `join`.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// How much engine output to hand back. A LaTeX log is mostly font chatter and
/// the useful part — the error and its line — is at the end.
const LOG_TAIL_BYTES: usize = 8 * 1024;

pub struct Compilation {
    /// What ran, for the UI: `latexmk (xelatex)`, `tectonic`, `pdflatex`.
    pub engine: String,
    /// Set when the toolchain could not honour the document's engine — the
    /// result may be wrong in ways the log will not mention.
    pub note: Option<String>,
    /// The PDF written next to the source, absent when none was produced.
    pub pdf: Option<PathBuf>,
    /// The engine reported errors. A PDF may still exist: TeX recovers from
    /// most of them, and a paper with a bad reference still beats no paper.
    pub had_errors: bool,
    pub log: String,
}

/// True when a file of that name sits on the shell's PATH — not the process's,
/// which in app mode is launchd's and has no TeX on it. That it runs is not
/// checked; `find_engine` probes on every `.tex` tab and spawning five engines
/// to ask their versions cost more than the case it caught.
fn on_path(binary: &str) -> bool {
    find_on_path(binary).is_some()
}

/// A TeX tool, with the shell's PATH in its environment: latexmk finds the
/// engine, biber and bibtex on PATH, so resolving latexmk alone is not enough.
fn tex_command(binary: &str) -> Command {
    // biber is never probed — it is chosen from what a pass wrote — so a bare
    // name here is what makes a machine without it fail as ENOENT at spawn.
    let mut command = match find_on_path(binary) {
        Some(path) => Command::new(path),
        None => Command::new(binary),
    };
    if let Some(paths) = search_path() {
        command.env("PATH", paths);
    }
    command
}

/// Pick how to compile a document written for `program`. Split from the PATH
/// lookup so the preference order is testable without a TeX install.
fn choose_driver(program: Program, available: &dyn Fn(&str) -> bool) -> Option<Driver> {
    // latexmk only helps if the engine it would drive is actually installed.
    if available("latexmk") && available(program.binary()) {
        return Some(Driver::Latexmk);
    }
    if available(program.binary()) {
        return Some(Driver::Direct);
    }
    if available("tectonic") {
        return Some(Driver::Tectonic);
    }
    None
}

/// The install hint when this specific document has no driver, else None. The
/// handler asks before compiling so the answer is a 400 naming what is missing
/// rather than a 500 — `find_engine` alone cannot tell, since a machine with
/// only xelatex can compile some documents and not others.
pub fn missing_toolchain(source: &Path) -> Option<String> {
    let program = read_head(source)
        .and_then(|head| program_from_source(&head))
        .unwrap_or(Program::Pdf);
    choose_driver(program, &|binary| on_path(binary))
        .is_none()
        .then(install_hint)
}

/// The head of a file, for the magic comment: bounded, since a `.tex` can be
/// large and only its first lines can carry one.
fn read_head(source: &Path) -> Option<String> {
    use std::io::Read;
    let file = std::fs::File::open(source).ok()?;
    let mut buf = Vec::new();
    file.take(4096).read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// A label for whatever this machine could compile with, or None when it has no
/// TeX at all. Used for the "can I compile here" probe, before any document.
pub fn find_engine() -> Option<&'static str> {
    ["latexmk", "tectonic", "pdflatex", "xelatex", "lualatex"]
        .into_iter()
        .find(|binary| on_path(binary))
}

/// Guidance for the no-engine case. Tectonic comes first because it is the one
/// a user can finish, and its XeTeX-only trade is named here because this is
/// the last screen that can say so — the hint is gone once an engine exists.
pub fn install_hint() -> String {
    let distribution = if cfg!(target_os = "macos") {
        "MacTeX"
    } else {
        "TeX Live"
    };
    format!(
        "No LaTeX toolchain found on PATH. Install Tectonic — one self-contained \
         binary that fetches each document's packages, though it runs only XeTeX. \
         For every engine and package, install {distribution} instead, which is \
         what Overleaf runs."
    )
}

/// A command the user can paste. Only offered for platforms whose command is
/// known to work — a wrong command pasted into a terminal is worse than none.
pub fn install_command() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        Some("brew install tectonic")
    } else {
        None
    }
}

/// Arguments for one pass, given an aux/output directory and the source file
/// name. The name is passed as `./name` so a file called `-foo.tex` or
/// `\input{..}.tex` reaches the engine as a path, not as options or as TeX to
/// execute.
fn pass_args(driver: Driver, program: Program, outdir: &Path, name: &str) -> Vec<String> {
    let outdir = outdir.to_string_lossy().to_string();
    let source = format!("./{name}");
    match driver {
        // -norc keeps latexmk from executing a .latexmkrc the agent wrote.
        // No -halt-on-error: it would make errors LaTeX recovers from fatal.
        Driver::Latexmk => vec![
            "-norc".into(),
            program.latexmk_flag().into(),
            "-interaction=nonstopmode".into(),
            "-no-shell-escape".into(),
            format!("-output-directory={outdir}"),
            source,
        ],
        Driver::Direct => vec![
            "-interaction=nonstopmode".into(),
            "-no-shell-escape".into(),
            format!("-output-directory={outdir}"),
            source,
        ],
        // Without continue-on-errors tectonic treats every TeX error as fatal,
        // including ones LaTeX itself recovers from — microtype under XeTeX
        // says "switching it off" and carries on everywhere else.
        Driver::Tectonic => vec![
            "--outdir".into(),
            outdir,
            "--keep-logs".into(),
            "-Z".into(),
            "continue-on-errors".into(),
            source,
        ],
    }
}

/// The binary a driver invokes.
fn driver_binary(driver: Driver, program: Program) -> &'static str {
    match driver {
        Driver::Latexmk => "latexmk",
        Driver::Direct => program.binary(),
        Driver::Tectonic => "tectonic",
    }
}

/// Engine passes once a bibliography tool has run — see the call site.
const BIBLIOGRAPHY_PASSES: usize = 3;

/// latexmk and tectonic repeat passes themselves. Driving an engine directly,
/// we do it: one pass to emit the `.aux`, then another for cross-references —
/// and a third when a bibliography tool ran in between.
fn pass_count(driver: Driver) -> usize {
    match driver {
        Driver::Direct => 2,
        _ => 1,
    }
}

/// The bibliography tool a first pass asked for, if any. biber announces itself
/// with a `.bcf` control file; classic bibtex writes `\bibdata` into the `.aux`.
fn bibliography_tool(outdir: &Path, stem: &str) -> Option<&'static str> {
    if outdir.join(format!("{stem}.bcf")).is_file() {
        return Some("biber");
    }
    let aux = std::fs::read_to_string(outdir.join(format!("{stem}.aux"))).ok()?;
    aux.contains("\\bibdata").then_some("bibtex")
}

/// Run biber/bibtex over the first pass's output. Its cwd is the aux directory
/// (both tools resolve their inputs relative to it), with `BIBINPUTS` pointing
/// back at the source so a `.bib` beside the paper is still found.
fn run_bibliography(
    tool: &'static str,
    outdir: &Path,
    source_dir: &Path,
    stem: &str,
) -> Result<Run> {
    // cwd is the aux dir (bibtex refuses to write outside it), so `.` no longer
    // means the paper's directory — both search paths have to say so.
    let search = format!("{}:", source_dir.to_string_lossy());
    let mut command = tex_command(tool);
    command
        .arg(stem)
        .current_dir(outdir)
        .env("BIBINPUTS", &search)
        .env("BSTINPUTS", &search);
    run_command(command, tool)
}

/// TeX marks every error with a line starting `!`. That, not the exit status,
/// is the signal: `continue-on-errors` makes tectonic exit 0 on a document it
/// only partly typeset, and a paper missing a command it never mentions is
/// exactly the silent lie this feature must not tell.
fn reports_errors(log: &str) -> bool {
    log.lines().any(|line| line.starts_with("! "))
}

fn tail(text: &str) -> String {
    if text.len() <= LOG_TAIL_BYTES {
        return text.to_string();
    }
    let start = text.len() - LOG_TAIL_BYTES;
    let start = (start..text.len())
        .find(|i| text.is_char_boundary(*i))
        .unwrap_or(text.len());
    format!("…\n{}", &text[start..])
}

/// SIGKILL the child's whole process group. `latexmk` runs the engine as a
/// grandchild that inherits the pipes, so killing only the direct child would
/// leave it spinning and hold the output readers open forever.
fn kill_process_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    if let Ok(group) = i32::try_from(child.id()) {
        // SAFETY: spawn put the child in its own group, whose id is its pid.
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
    }
    let _ = child.kill();
}

/// Run one pass to completion, killing its process tree if it outlives the
/// deadline. `-interaction=nonstopmode` plus a null stdin keeps a broken
/// document off TeX's `?` prompt; the timeout is for a package loop.
/// One tool invocation. `timed_out` is kept apart from `ok` because they call
/// for opposite responses: a TeX error is worth continuing past, a timeout is
/// not — each remaining pass would spend the same budget again.
struct Run {
    ok: bool,
    timed_out: bool,
    log: String,
}

fn run_pass(binary: &str, dir: &Path, args: &[String]) -> Result<Run> {
    let mut command = tex_command(binary);
    command.args(args).current_dir(dir);
    run_command(command, binary)
}

/// Run one tool to completion, killing its process tree if it outlives the
/// deadline, and return whether it exited clean plus everything it printed.
fn run_command(command: Command, label: &str) -> Result<Run> {
    run_with_timeout(command, label, PASS_TIMEOUT)
}

/// The runner proper. The timeout is a parameter so the kill path can be tested
/// without waiting out a real pass budget.
fn run_with_timeout(mut command: Command, label: &str, timeout: Duration) -> Result<Run> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|e| anyhow!("could not run {label}: {e}"))?;

    // Drain both pipes on their own threads: a pass that fills one while we sit
    // in the wait loop would otherwise deadlock against its own output.
    let (tx, rx) = mpsc::channel();
    for (stream, is_stdout) in [
        (child.stdout.take().map(Stream::Out), true),
        (child.stderr.take().map(Stream::Err), false),
    ] {
        let Some(stream) = stream else { continue };
        let tx = tx.clone();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = Vec::new();
            let read = match stream {
                Stream::Out(mut s) => s.read_to_end(&mut buf),
                Stream::Err(mut s) => s.read_to_end(&mut buf),
            };
            let _ = tx.send((is_stdout, read.map(|_| buf).unwrap_or_default()));
        });
    }
    drop(tx);

    let deadline = Instant::now() + timeout;
    let (status, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (status, false),
            Ok(None) => {}
            Err(e) => {
                kill_process_tree(&mut child);
                let _ = child.wait();
                return Err(anyhow!("could not wait for {label}: {e}"));
            }
        }
        if Instant::now() >= deadline {
            kill_process_tree(&mut child);
            break (child.wait()?, true);
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    let mut out = Vec::new();
    let mut err = Vec::new();
    let drain_by = Instant::now() + DRAIN_TIMEOUT;
    while let Ok((is_stdout, buf)) =
        rx.recv_timeout(drain_by.saturating_duration_since(Instant::now()))
    {
        if is_stdout {
            out = buf;
        } else {
            err = buf;
        }
    }

    let mut log = String::from_utf8_lossy(&out).into_owned();
    log.push_str(&String::from_utf8_lossy(&err));
    if timed_out {
        log.push_str(&format!(
            "\n{label} timed out after {} seconds and was stopped.\n",
            timeout.as_secs()
        ));
    }
    Ok(Run {
        ok: status.success() && !timed_out,
        timed_out,
        log,
    })
}

enum Stream {
    Out(std::process::ChildStdout),
    Err(std::process::ChildStderr),
}

/// Unique per call. The counter is what actually guarantees it — two threads
/// can read the same nanosecond, and this name is the only thing keeping
/// concurrent writers off each other's staging file.
fn unique_suffix() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{}-{nanos}-{n}", std::process::id())
}

/// A scratch directory for the aux files, removed when the guard drops so a
/// failed compile leaves nothing behind.
struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new(stem: &str) -> Result<Self> {
        // Nanos + pid keep concurrent compiles of the same file apart.
        let unique = unique_suffix();
        let dir = std::env::temp_dir().join(format!(
            "orx-latex-{}-{unique}",
            crate::local::slugify(stem)
        ));
        std::fs::create_dir(&dir)
            .map_err(|e| anyhow!("could not create a scratch directory: {e}"))?;
        Ok(Self(dir))
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Compile `source` with `engine` and, on success, write the PDF beside it. Aux
/// files stay in a scratch directory so a compile never litters the repository.
pub fn compile(source: &Path) -> Result<Compilation> {
    let dir = source
        .parent()
        .ok_or_else(|| anyhow!("file has no parent directory"))?;
    let name = source
        .file_name()
        .ok_or_else(|| anyhow!("file has no name"))?
        .to_string_lossy()
        .into_owned();
    let stem = source
        .file_stem()
        .ok_or_else(|| anyhow!("file has no name"))?
        .to_string_lossy()
        .into_owned();

    // Only an explicit request can be unhonoured: with no magic comment the
    // document expressed no preference, and warning about the default would
    // put an amber banner on every paper a tectonic-only machine compiles.
    let requested = read_head(source).and_then(|head| program_from_source(&head));
    let program = requested.unwrap_or(Program::Pdf);
    let driver =
        choose_driver(program, &|binary| on_path(binary)).ok_or_else(|| anyhow!(install_hint()))?;
    let binary = driver_binary(driver, program);
    let engine = match driver {
        Driver::Latexmk => format!("latexmk ({})", program.binary()),
        _ => binary.to_string(),
    };
    // tectonic is XeTeX whatever the document asked for. Compiling anyway beats
    // refusing, but the result can differ from what the author intended.
    let unhonoured = requested.is_some_and(|p| !p.served_by_tectonic());
    let note = (driver == Driver::Tectonic && unhonoured).then(|| {
        format!(
            "This document asks for {}, but only Tectonic is installed and it runs XeTeX. \
             Install a full TeX distribution to compile it as written.",
            program.binary()
        )
    });

    let scratch = ScratchDir::new(&stem)?;
    mirror_input_directories(source, &scratch.0);
    let args = pass_args(driver, program, &scratch.0, &name);

    let mut log = String::new();
    // Kept for the end of the transcript, where `tail` cannot trim them away.
    let mut warnings: Vec<String> = Vec::new();
    let mut clean = true;
    let mut passes = pass_count(driver);
    let mut pass = 0;
    while pass < passes {
        let run = run_pass(binary, dir, &args)?;
        log.push_str(&run.log);
        // TeX exits non-zero for any error it merely recovered from, so a
        // stray macro typo must not stop the remaining passes: doing that
        // leaves every \ref reading "??" and skips the bibliography entirely.
        clean &= run.ok;
        // A timeout is the exception: continuing spends the whole budget again
        // on each remaining pass, and nothing downstream can cancel it.
        if run.timed_out {
            break;
        }
        // Driving the engine ourselves, the bibliography runs between passes.
        if pass == 0 && driver == Driver::Direct {
            if let Some(tool) = bibliography_tool(&scratch.0, &stem) {
                match run_bibliography(tool, &scratch.0, dir, &stem) {
                    Ok(run) => {
                        log.push_str(&run.log);
                        if !run.ok {
                            clean = false;
                            warnings.push(format!("{tool} failed — citations will not resolve."));
                        }
                    }
                    // Not installed is a routine gap (biber ships separately
                    // from TeX Live). The document still has a readable PDF, so
                    // say what is missing rather than failing the whole compile.
                    Err(e) => {
                        clean = false;
                        // run_command already says "could not run <tool>: …".
                        warnings.push(format!("{e} — citations will not resolve."));
                    }
                }
                // latex → bib → latex → latex: the first rerun pulls the .bbl
                // in, the second settles the labels it renumbered. Stopping at
                // one leaves every \cite reading "[?]" until a second compile.
                passes = BIBLIOGRAPHY_PASSES;
            }
        }
        pass += 1;
    }

    // The .log file says what happened far more precisely than the console
    // transcript, so it goes last — but appended, not substituted: replacing
    // would delete the bibliography transcript and any timeout notice, which
    // are often the only record of why a PDF came out wrong.
    if let Ok(bytes) = std::fs::read(scratch.0.join(format!("{stem}.log"))) {
        log.push('\n');
        log.push_str(&String::from_utf8_lossy(&bytes));
    }
    for warning in &warnings {
        log.push('\n');
        log.push_str(warning);
        log.push('\n');
    }
    // Detect before tailing — an error early in a long log would be cut off.
    let had_errors = !clean || reports_errors(&log);

    let produced = scratch.0.join(format!("{stem}.pdf"));
    if !produced.is_file() {
        return Ok(Compilation {
            engine,
            note,
            pdf: None,
            had_errors: true,
            log: tail(&log),
        });
    }

    let destination = write_pdf_beside(source, &produced)?;
    Ok(Compilation {
        engine,
        note,
        pdf: Some(destination),
        had_errors,
        log: tail(&log),
    })
}

/// LaTeX writes an `\include`d file's `.aux` under the output directory, mirroring
/// the source path — and aborts if the directory is not there. Create the ones
/// the document names rather than mirroring a whole repository.
fn mirror_input_directories(source: &Path, outdir: &Path) {
    let Ok(text) = std::fs::read_to_string(source) else {
        return;
    };
    for command in ["\\include{", "\\input{"] {
        for piece in text.split(command).skip(1) {
            let Some((arg, _)) = piece.split_once('}') else {
                continue;
            };
            let Some((dir, _)) = arg.trim().rsplit_once('/') else {
                continue;
            };
            let nested = outdir.join(dir);
            // The argument comes from the document; keep it under the scratch dir.
            if nested.starts_with(outdir) && !dir.contains("..") {
                let _ = std::fs::create_dir_all(&nested);
            }
        }
    }
}

/// Publish the compiled PDF next to its source, the way a command-line compile
/// would. Written through a sibling temp file and renamed so a reader never
/// sees a half-copied PDF.
fn write_pdf_beside(source: &Path, produced: &Path) -> Result<PathBuf> {
    let destination = source.with_extension("pdf");
    // `fs::copy` follows a symlink at the destination, so a repo carrying
    // `paper.pdf -> ~/.zshrc` would have that file overwritten. The source is
    // confined to the checkout; a symlink is the one way out of it.
    if destination
        .symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink())
    {
        return Err(anyhow!(
            "{} is a symlink — refusing to overwrite it",
            destination.display()
        ));
    }
    // Unique per write: two compiles of the same file overlap easily (a save
    // recompile landing on an open tab's, or two tabs on one paper). With a
    // fixed staging name the first rename moves the file out from under the
    // second, which then fails ENOENT having already "compiled".
    let staged = destination.with_extension(format!("pdf.{}.orx-tmp", unique_suffix()));
    std::fs::copy(produced, &staged)
        .map_err(|e| anyhow!("compiled, but could not write {}: {e}", staged.display()))?;
    if let Err(e) = std::fs::rename(&staged, &destination) {
        let _ = std::fs::remove_file(&staged);
        return Err(anyhow!(
            "compiled, but could not write {}: {e}",
            destination.display()
        ));
    }
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_driver_writes_its_aux_to_the_scratch_directory() {
        let outdir = Path::new("/tmp/scratch");
        for driver in [Driver::Latexmk, Driver::Direct, Driver::Tectonic] {
            let args = pass_args(driver, Program::Pdf, outdir, "paper.tex");
            assert!(
                args.iter().any(|a| a.contains("/tmp/scratch")),
                "{driver:?} must be told where to put aux files"
            );
            assert_eq!(
                args.last().map(String::as_str),
                Some("./paper.tex"),
                "{driver:?} takes the source last, path-qualified"
            );
        }
        assert_eq!(
            pass_count(Driver::Direct),
            2,
            "cross-refs need a second pass"
        );
        assert_eq!(pass_count(Driver::Latexmk), 1, "latexmk repeats itself");
    }

    #[test]
    fn a_hostile_file_name_cannot_become_options_or_tex_source() {
        for name in ["-output-directory=x.tex", "\\immediate\\write18{sh}.tex"] {
            let args = pass_args(Driver::Direct, Program::Pdf, Path::new("/tmp/x"), name);
            let source = args.last().expect("source argument");
            assert!(source.starts_with("./"), "{name} must stay a path");
            assert!(!source.starts_with('-') && !source.starts_with('\\'));
        }
    }

    #[test]
    fn batch_drivers_run_non_interactively_without_shell_escape() {
        for driver in [Driver::Latexmk, Driver::Direct] {
            let args = pass_args(driver, Program::Pdf, Path::new("/tmp/x"), "paper.tex");
            assert!(
                args.iter().any(|a| a == "-interaction=nonstopmode"),
                "{driver:?} must run non-interactively"
            );
            assert!(
                args.iter().any(|a| a == "-no-shell-escape"),
                "{driver:?} compiles agent-authored source; shell escape stays off"
            );
            assert!(
                !args.iter().any(|a| a == "-halt-on-error"),
                "{driver:?} must survive errors LaTeX itself recovers from"
            );
        }
        assert!(
            pass_args(Driver::Latexmk, Program::Pdf, Path::new("/tmp/x"), "p.tex")
                .iter()
                .any(|a| a == "-norc"),
            "latexmk must not execute a .latexmkrc from the checkout"
        );
    }

    #[test]
    fn latexmk_is_told_which_engine_the_document_wants() {
        for (program, flag, binary) in [
            (Program::Pdf, "-pdf", "pdflatex"),
            (Program::Xe, "-xelatex", "xelatex"),
            (Program::Lua, "-lualatex", "lualatex"),
        ] {
            let args = pass_args(Driver::Latexmk, program, Path::new("/tmp/x"), "p.tex");
            assert!(args.iter().any(|a| a == flag), "{program:?} wants {flag}");
            assert_eq!(driver_binary(Driver::Direct, program), binary);
        }
    }

    #[test]
    fn the_program_comment_is_read_in_the_forms_authors_actually_write() {
        let cases = [
            (
                "% !TeX program = lualatex\n\\documentclass{article}",
                Some(Program::Lua),
            ),
            ("%!TEX TS-program = xelatex\n", Some(Program::Xe)),
            ("%% !tex program=pdflatex\n", Some(Program::Pdf)),
            ("  % !TeX program = XeLaTeX\n", Some(Program::Xe)),
            (
                "\\documentclass{article}\n% !TeX program = lualatex\n",
                Some(Program::Lua),
            ),
            ("% !TeX spellcheck = en_GB\n", None),
            // Papers stack these; an earlier directive must not stop the scan.
            (
                "% !TeX root = main.tex\n% !TeX program = lualatex\n",
                Some(Program::Lua),
            ),
            (
                "% !TeX encoding = UTF-8\n%!BIB program = biber\n% !TeX program = xelatex\n",
                Some(Program::Xe),
            ),
            ("% !TeX program = knuthtex\n", None),
            ("\\documentclass{article}\n", None),
        ];
        for (source, want) in cases {
            assert_eq!(program_from_source(source), want, "for {source:?}");
        }
        // Far enough down the file, a matching line is body text, not a header.
        let buried = "x\n".repeat(PROGRAM_COMMENT_LINES) + "% !TeX program = lualatex\n";
        assert_eq!(program_from_source(&buried), None);
    }

    #[test]
    fn the_driver_prefers_latexmk_then_the_engine_then_tectonic() {
        let have = |names: &'static [&'static str]| move |binary: &str| names.contains(&binary);
        let all = have(&["latexmk", "pdflatex", "xelatex", "lualatex", "tectonic"]);
        assert_eq!(choose_driver(Program::Lua, &all), Some(Driver::Latexmk));

        let no_latexmk = have(&["pdflatex", "xelatex", "tectonic"]);
        assert_eq!(
            choose_driver(Program::Xe, &no_latexmk),
            Some(Driver::Direct)
        );
        // LuaLaTeX is not installed, so tectonic stands in — see the note it carries.
        assert_eq!(
            choose_driver(Program::Lua, &no_latexmk),
            Some(Driver::Tectonic)
        );

        let nothing = have(&[]);
        assert_eq!(choose_driver(Program::Pdf, &nothing), None);
    }

    #[test]
    fn only_an_explicit_engine_request_can_go_unhonoured() {
        // No magic comment: the document asked for nothing, so a tectonic-only
        // machine has nothing to apologise for.
        assert_eq!(program_from_source("\\documentclass{article}"), None);
        assert_eq!(
            program_from_source("% !TeX program = lualatex\n"),
            Some(Program::Lua)
        );
    }

    #[test]
    fn tectonic_can_only_stand_in_for_xetex() {
        assert!(Program::Xe.served_by_tectonic());
        assert!(!Program::Lua.served_by_tectonic());
        assert!(!Program::Pdf.served_by_tectonic());
    }

    #[test]
    fn the_bibliography_tool_is_read_from_what_the_first_pass_wrote() {
        let scratch = ScratchDir::new("bib").expect("scratch");
        assert_eq!(bibliography_tool(&scratch.0, "paper"), None);

        std::fs::write(scratch.0.join("paper.aux"), "\\relax\n\\bibdata{refs}\n").expect("aux");
        assert_eq!(bibliography_tool(&scratch.0, "paper"), Some("bibtex"));

        // biblatex writes a .bcf, and biber takes precedence over bibtex.
        std::fs::write(scratch.0.join("paper.bcf"), "<control/>").expect("bcf");
        assert_eq!(bibliography_tool(&scratch.0, "paper"), Some("biber"));
    }

    #[test]
    fn the_no_engine_guidance_points_at_tectonic_first() {
        let hint = install_hint();
        let distribution = if cfg!(target_os = "macos") {
            "MacTeX"
        } else {
            "TeX Live"
        };
        let tectonic = hint.find("Tectonic").expect("the recommendation is named");
        let fallback = hint.find(distribution).expect("the fallback is named");
        assert!(
            tectonic < fallback,
            "the install a user can finish comes first"
        );
        // The caveat has to sit in Tectonic's own clause: recommending it
        // without one sends a user to an engine that silently retypesets their
        // pdfLaTeX document, and this hint is the only place that says so.
        let xetex = hint.find("XeTeX").expect("the limitation is named");
        assert!(tectonic < xetex && xetex < fallback);
        // The prose must not duplicate the copyable command, but the command
        // must install what the prose leads with.
        assert!(!hint.contains("brew install"));
        #[cfg(target_os = "macos")]
        assert_eq!(install_command(), Some("brew install tectonic"));
        #[cfg(not(target_os = "macos"))]
        assert_eq!(install_command(), None);
    }

    #[test]
    fn a_tex_tool_is_handed_the_path_its_own_children_need() {
        // `sh` stands in for a TeX tool; all that matters is that it is on PATH.
        let command = tex_command("sh");
        let (_, value) = command
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .expect("the child is given an explicit PATH");
        assert_eq!(value, search_path().as_deref());
        // And the tool itself is resolved, not left to the child's own lookup.
        assert!(Path::new(command.get_program()).is_absolute());
    }

    #[test]
    fn tectonic_does_not_treat_recoverable_errors_as_fatal() {
        // microtype under XeTeX errors with "switching it off" and carries on
        // everywhere else; without this flag tectonic produces no PDF at all.
        let args = pass_args(Driver::Tectonic, Program::Pdf, Path::new("/tmp/x"), "p.tex");
        let joined = args.join(" ");
        assert!(joined.contains("-Z continue-on-errors"), "got {joined}");
    }

    #[test]
    fn concurrent_writes_of_one_pdf_do_not_clobber_each_other() {
        let scratch = ScratchDir::new("race").expect("scratch dir");
        let source = scratch.0.join("paper.tex");
        std::fs::write(&source, b"\\documentclass{article}").expect("write");

        let handles: Vec<_> = (0..8)
            .map(|i| {
                let produced = scratch.0.join(format!("built-{i}.pdf"));
                std::fs::write(&produced, format!("%PDF-1.4 build {i}")).expect("write");
                let source = source.clone();
                std::thread::spawn(move || write_pdf_beside(&source, &produced))
            })
            .collect();
        for handle in handles {
            handle
                .join()
                .expect("thread")
                .expect("every write must land");
        }

        // One complete PDF, and no staging files left behind.
        let written = std::fs::read_to_string(scratch.0.join("paper.pdf")).expect("read");
        assert!(
            written.starts_with("%PDF-1.4 build "),
            "torn file: {written}"
        );
        let leftovers: Vec<_> = std::fs::read_dir(&scratch.0)
            .expect("read scratch")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("orx-tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "staging files survived: {leftovers:?}"
        );
    }

    #[test]
    fn the_staging_suffix_is_unique_under_concurrency() {
        let handles: Vec<_> = (0..16)
            .map(|_| std::thread::spawn(|| (0..64).map(|_| unique_suffix()).collect::<Vec<_>>()))
            .collect();
        let all: Vec<String> = handles
            .into_iter()
            .flat_map(|h| h.join().expect("thread"))
            .collect();
        let unique: std::collections::HashSet<&String> = all.iter().collect();
        assert_eq!(unique.len(), all.len(), "suffixes collided");
    }

    #[test]
    fn an_included_subdirectory_is_created_in_the_scratch_dir() {
        let scratch = ScratchDir::new("inc").expect("scratch");
        let source = scratch.0.join("paper.tex");
        std::fs::write(
            &source,
            "\\include{sections/intro}\n\\input{tables/results}\n\\input{plain}\n",
        )
        .expect("write");
        let out = ScratchDir::new("inc-out").expect("scratch");
        mirror_input_directories(&source, &out.0);
        assert!(out.0.join("sections").is_dir());
        assert!(out.0.join("tables").is_dir());
        assert!(
            !out.0.join("plain").exists(),
            "no directory for a bare name"
        );
    }

    #[test]
    fn an_include_path_cannot_escape_the_scratch_dir() {
        let scratch = ScratchDir::new("esc").expect("scratch");
        let source = scratch.0.join("paper.tex");
        std::fs::write(&source, "\\input{../../../etc/evil/x}\n").expect("write");
        let out = ScratchDir::new("esc-out").expect("scratch");
        mirror_input_directories(&source, &out.0);
        assert!(!out.0.join("../../../etc/evil").exists());
    }

    #[test]
    fn a_timed_out_pass_stops_the_run_but_a_tex_error_does_not() {
        // A hanging engine must not spend the whole budget again on every
        // remaining pass; a document that merely errored must still finish.
        let scratch = ScratchDir::new("timeout").expect("scratch");
        let script = scratch.0.join("hang.sh");
        std::fs::write(&script, "#!/bin/sh\nsleep 30\n").expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }
        let mut command = Command::new(&script);
        command.current_dir(&scratch.0);
        // Borrow the real runner but with a deadline we can wait out.
        let started = Instant::now();
        let run = run_with_timeout(command, "hang", Duration::from_millis(300)).expect("run");
        assert!(run.timed_out, "the runner must report the timeout");
        assert!(!run.ok);
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "killed promptly"
        );
    }

    #[test]
    fn tex_errors_are_read_from_the_log_not_the_exit_status() {
        assert!(reports_errors(
            "Overfull hbox\n! Undefined control sequence.\nl.4 \\foo"
        ));
        assert!(reports_errors("! LaTeX Error: File `x.sty' not found."));
        assert!(!reports_errors(
            "LaTeX Warning: Citation `a' undefined.\nPackage caption Info: fine.\n"
        ));
        // Prose that merely contains an exclamation mark is not an error line.
        assert!(!reports_errors(
            "Output written on paper.pdf (3 pages).\nDone!\n"
        ));
    }

    #[test]
    fn tail_keeps_the_end_and_splits_on_a_char_boundary() {
        assert_eq!(tail("short"), "short");
        let long = "é".repeat(LOG_TAIL_BYTES);
        let tailed = tail(&long);
        assert!(tailed.starts_with("…\n"));
        assert!(tailed.ends_with('é'));
        assert!(long.ends_with(tailed.trim_start_matches("…\n")));
    }

    #[test]
    fn the_scratch_directory_is_removed_on_drop() {
        let scratch = ScratchDir::new("paper").expect("scratch dir");
        let path = scratch.0.clone();
        assert!(path.is_dir());
        drop(scratch);
        assert!(!path.exists());
    }

    #[test]
    fn a_symlinked_pdf_is_refused_instead_of_followed() {
        let scratch = ScratchDir::new("symlink-case").expect("scratch dir");
        let outside = scratch.0.join("secret");
        std::fs::write(&outside, b"keep me").expect("write");
        let source = scratch.0.join("paper.tex");
        std::fs::write(&source, b"\\documentclass{article}").expect("write");
        let produced = scratch.0.join("built.pdf");
        std::fs::write(&produced, b"%PDF-1.4").expect("write");

        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, scratch.0.join("paper.pdf")).expect("symlink");

        let err = write_pdf_beside(&source, &produced).expect_err("must refuse a symlink");
        assert!(err.to_string().contains("symlink"));
        assert_eq!(std::fs::read(&outside).expect("read"), b"keep me");
    }

    #[test]
    fn a_regular_pdf_is_replaced_in_place() {
        let scratch = ScratchDir::new("replace-case").expect("scratch dir");
        let source = scratch.0.join("paper.tex");
        std::fs::write(&source, b"\\documentclass{article}").expect("write");
        std::fs::write(scratch.0.join("paper.pdf"), b"stale").expect("write");
        let produced = scratch.0.join("built.pdf");
        std::fs::write(&produced, b"%PDF-1.4 fresh").expect("write");

        let written = write_pdf_beside(&source, &produced).expect("write pdf");
        assert_eq!(written, scratch.0.join("paper.pdf"));
        assert_eq!(std::fs::read(&written).expect("read"), b"%PDF-1.4 fresh");
        let leftovers: Vec<_> = std::fs::read_dir(&scratch.0)
            .expect("read scratch")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("orx-tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "staging files survived: {leftovers:?}"
        );
    }
}
