//! Google Colab backend — a local controller drives one Colab runtime per run
//! through the official `colab` CLI (google-colab-cli).
//!
//! The run is a local job (`colab_job`, same run-dir layout as `localbox`):
//! its `run.sh` starts `colab-controller.sh`, which
//!   1. assigns a runtime (`colab new --gpu <G>`),
//!   2. uploads the immutable source snapshot (`colab upload`),
//!   3. pipes `colab-driver.py` into `colab exec`; the driver extracts the
//!      snapshot, runs the fixed command, streams its output, and prints a
//!      sentinel with the exit code (`colab exec` does not propagate one),
//!   4. releases the runtime on every exit path (`colab stop`), including
//!      cancellation, which TERMs the controller's process group.
//!
//! Secrets never touch disk or argv: the controller reads them from its own
//! environment and pipes them, base64-encoded, into the kernel over stdin.

use std::path::{Path, PathBuf};

use crate::error::{anyhow, Result};

/// One assignable Colab accelerator, as `colab new` accepts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Accelerator {
    /// orx flavor id (lowercase).
    pub id: &'static str,
    /// `gpu`, `tpu`, or `cpu`.
    pub kind: &'static str,
    /// The value passed to `colab new --gpu/--tpu`.
    pub cli_value: &'static str,
    pub label: &'static str,
    /// Accelerator memory (GPU VRAM or TPU HBM) in GB.
    pub memory_gb: u32,
    /// Colab offers a high-RAM machine shape for this accelerator.
    pub high_mem: bool,
}

/// Ordered by accelerator memory, so the first match is the smallest fit.
pub const ACCELERATORS: &[Accelerator] = &[
    Accelerator {
        id: "cpu",
        kind: "cpu",
        cli_value: "",
        label: "CPU only",
        memory_gb: 0,
        high_mem: true,
    },
    Accelerator {
        id: "t4",
        kind: "gpu",
        cli_value: "T4",
        label: "NVIDIA T4",
        memory_gb: 16,
        high_mem: true,
    },
    Accelerator {
        id: "l4",
        kind: "gpu",
        cli_value: "L4",
        label: "NVIDIA L4",
        memory_gb: 24,
        high_mem: false,
    },
    Accelerator {
        id: "a100",
        kind: "gpu",
        cli_value: "A100",
        label: "NVIDIA A100",
        memory_gb: 40,
        high_mem: true,
    },
    Accelerator {
        id: "h100",
        kind: "gpu",
        cli_value: "H100",
        label: "NVIDIA H100",
        memory_gb: 80,
        high_mem: true,
    },
    Accelerator {
        id: "g4",
        kind: "gpu",
        cli_value: "G4",
        label: "NVIDIA RTX PRO 6000 (G4)",
        memory_gb: 96,
        high_mem: true,
    },
    Accelerator {
        id: "v5e1",
        kind: "tpu",
        cli_value: "v5e1",
        label: "TPU v5e (1 chip)",
        memory_gb: 16,
        high_mem: false,
    },
    Accelerator {
        id: "v6e1",
        kind: "tpu",
        cli_value: "v6e1",
        label: "TPU v6e (1 chip)",
        memory_gb: 32,
        high_mem: false,
    },
];

/// The flavor used when a launch names none: the cheapest GPU.
pub const DEFAULT_FLAVOR: &str = "t4";

const HIGH_MEM_SUFFIXES: &[&str] = &["highmem", "high-mem", "hm"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flavor {
    pub accelerator: Accelerator,
    pub high_mem: bool,
}

impl Flavor {
    /// The canonical spelling, e.g. `a100:highmem`.
    pub fn id(&self) -> String {
        if self.high_mem {
            format!("{}:highmem", self.accelerator.id)
        } else {
            self.accelerator.id.to_string()
        }
    }

    pub fn label(&self) -> String {
        let a = &self.accelerator;
        let mut label = if a.memory_gb > 0 {
            format!("{} ({} GB)", a.label, a.memory_gb)
        } else {
            a.label.to_string()
        };
        if self.high_mem {
            label.push_str(", high-RAM");
        }
        label
    }

    /// Arguments for `colab new`.
    pub fn cli_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        match self.accelerator.kind {
            "gpu" => args.extend(["--gpu".to_string(), self.accelerator.cli_value.to_string()]),
            "tpu" => args.extend(["--tpu".to_string(), self.accelerator.cli_value.to_string()]),
            _ => {}
        }
        if self.high_mem {
            args.push("--high-mem".to_string());
        }
        args
    }
}

/// Parse `--flavor` for `--backend colab`.
///
/// - `t4`, `l4`, `a100`, `h100`, `g4`, `v5e1`, `v6e1`, or `cpu`;
/// - `<id>:highmem` for a high-RAM machine shape (not L4 or TPUs);
/// - `auto:<GB>` for the smallest GPU with at least that much VRAM.
///
/// Omitted means [`DEFAULT_FLAVOR`]. The `colab` CLI maps unknown GPU names to
/// an A100 silently, so anything else is rejected here.
pub fn parse_flavor(flavor: Option<&str>) -> Result<Flavor> {
    let raw = flavor
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .unwrap_or(DEFAULT_FLAVOR)
        .to_ascii_lowercase();
    let (name, modifier) = match raw.split_once(':') {
        Some((name, modifier)) => (name.trim(), Some(modifier.trim())),
        None => (raw.as_str(), None),
    };
    if name == "auto" {
        let needed = modifier
            .and_then(|gb| gb.trim_end_matches("gb").trim().parse::<f64>().ok())
            .filter(|gb| gb.is_finite() && *gb > 0.0)
            .ok_or_else(|| {
                anyhow!("--flavor auto needs the VRAM the run needs in GB, e.g. auto:24.")
            })?;
        let accelerator = smallest_gpu_for(needed).ok_or_else(|| {
            anyhow!(
                "No Colab GPU has {needed} GB of VRAM (the largest has {} GB). \
                 Shrink the batch, quantize, or use a multi-GPU backend.",
                largest_gpu().memory_gb
            )
        })?;
        return Ok(Flavor {
            accelerator,
            high_mem: false,
        });
    }
    let accelerator = *ACCELERATORS
        .iter()
        .find(|a| a.id == name)
        .ok_or_else(|| anyhow!("Unknown Colab flavor '{raw}'. {}", flavor_help()))?;
    let high_mem = match modifier {
        None => false,
        Some(m) if HIGH_MEM_SUFFIXES.contains(&m) => true,
        Some(_) => return Err(anyhow!("Unknown Colab flavor '{raw}'. {}", flavor_help())),
    };
    if high_mem && !accelerator.high_mem {
        return Err(anyhow!(
            "Colab offers only one machine shape for {}; drop `:highmem`.",
            accelerator.label
        ));
    }
    Ok(Flavor {
        accelerator,
        high_mem,
    })
}

fn flavor_help() -> String {
    let ids: Vec<_> = ACCELERATORS.iter().map(|a| a.id).collect();
    format!(
        "Use one of {}, optionally with `:highmem`, or `auto:<GB of VRAM>`.",
        ids.join(", ")
    )
}

/// The smallest GPU whose VRAM covers `needed_gb`.
pub fn smallest_gpu_for(needed_gb: f64) -> Option<Accelerator> {
    ACCELERATORS
        .iter()
        .filter(|a| a.kind == "gpu")
        .find(|a| f64::from(a.memory_gb) >= needed_gb)
        .copied()
}

fn largest_gpu() -> Accelerator {
    *ACCELERATORS
        .iter()
        .filter(|a| a.kind == "gpu")
        .max_by_key(|a| a.memory_gb)
        .expect("at least one Colab GPU")
}

/// Where google-colab-cli keeps its OAuth token after `colab` signs in.
pub fn token_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".config/colab-cli/token.json"))
}

/// The `colab` executable: PATH first, then uv/pipx's `~/.local/bin`.
pub fn find_cli() -> Option<PathBuf> {
    crate::local::shell_env::find_on_path("colab").or_else(|| {
        dirs::home_dir().and_then(|home| {
            crate::local::shell_env::find_in_dir(&home.join(".local/bin"), "colab")
        })
    })
}

pub const INSTALL_HINT: &str =
    "Install the Colab CLI with `uv tool install google-colab-cli` (or `pip install google-colab-cli`).";
pub const SIGN_IN_HINT: &str =
    "Sign in once by running `colab sessions` in a terminal (or `orx compute connect colab`).";

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub supported_os: bool,
    pub cli_path: Option<String>,
    pub cli_version: Option<String>,
    pub signed_in: bool,
    pub ready: bool,
    pub error: Option<String>,
    pub default_flavor: &'static str,
    pub accelerators: &'static [Accelerator],
}

/// Readiness without touching the network or starting a sign-in flow.
pub async fn status() -> Status {
    let supported_os = cfg!(unix);
    let cli = find_cli();
    let cli_version = match &cli {
        Some(path) => cli_version(path).await,
        None => None,
    };
    let signed_in = token_path().is_some_and(|path| path.is_file());
    let error = if !supported_os {
        Some("The Colab CLI supports Linux and macOS only.".to_string())
    } else if cli.is_none() {
        Some(INSTALL_HINT.to_string())
    } else if !signed_in {
        Some(SIGN_IN_HINT.to_string())
    } else {
        None
    };
    Status {
        supported_os,
        cli_path: cli.map(|p| p.to_string_lossy().into_owned()),
        cli_version,
        signed_in,
        ready: error.is_none(),
        error,
        default_flavor: DEFAULT_FLAVOR,
        accelerators: ACCELERATORS,
    }
}

async fn cli_version(path: &Path) -> Option<String> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        tokio::process::Command::new(path)
            .arg("version")
            .stdin(std::process::Stdio::null())
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

/// The executable when Colab is usable, else the reason it isn't.
pub async fn preflight() -> Result<PathBuf> {
    let status = status().await;
    match (status.error, status.cli_path) {
        (None, Some(path)) => Ok(PathBuf::from(path)),
        (Some(error), _) => Err(anyhow!("{error}")),
        (None, None) => Err(anyhow!("{INSTALL_HINT}")),
    }
}

/// A stable, short Colab session name for a run.
pub fn session_name(run_id: &str) -> String {
    let id: String = run_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(12)
        .collect();
    format!("orx-{id}")
}

/// Default wall clock for a Colab run, matching the other remote backends.
pub const DEFAULT_TIMEOUT_SECS: u64 = 4 * 3600;

pub const CONTROLLER_FILE: &str = "colab-controller.sh";
pub const DRIVER_FILE: &str = "colab-driver.py";
const EXIT_SENTINEL: &str = "__ORX_EXIT_CODE__=";

pub struct ControllerSpec<'a> {
    pub cli: &'a Path,
    pub session: &'a str,
    pub flavor: &'a Flavor,
    pub archive: &'a Path,
    pub timeout_secs: u64,
    /// Names of environment variables forwarded to the run. Values are read
    /// from the controller's environment at run time.
    pub env_keys: Vec<String>,
}

fn is_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Name of the snapshot on the runtime; content-addressed so a retry reuses it.
fn remote_archive_name(archive: &Path) -> String {
    let stem = archive
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let digest: String = stem
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(16)
        .collect();
    format!("orx-source-{digest}.tar")
}

/// The bash controller that drives one run (see the module docs).
pub fn controller_script(spec: &ControllerSpec<'_>) -> String {
    use crate::jobs::ssh::sh_quote;
    let new_args = spec
        .flavor
        .cli_args()
        .iter()
        .map(|arg| sh_quote(arg))
        .collect::<Vec<_>>()
        .join(" ");
    let env_keys = spec
        .env_keys
        .iter()
        .filter(|key| is_env_name(key))
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    // `colab exec --timeout` bounds the whole cell; the driver enforces the
    // run's own limit first, so give the CLI a little slack.
    let exec_timeout = spec.timeout_secs + 600;
    format!(
        r#"#!/usr/bin/env bash
# Generated by orx: runs one experiment on a Google Colab runtime.
set -u
shopt -s lastpipe
colab={cli}
session={session}
released=0
release() {{
  if [ "$released" = 0 ]; then
    released=1
    echo "[orx] Releasing Colab runtime $session"
    "$colab" stop -s "$session" >/dev/null 2>&1 \
      || echo "[orx] Could not release $session; run: colab stop -s $session"
  fi
}}
trap release EXIT
trap 'exit 143' TERM INT HUP

echo "[orx] Requesting a Colab runtime: {label}"
if ! "$colab" new -s "$session" {new_args} </dev/null; then
  echo "[orx] Colab could not assign this runtime. The accelerator may be unavailable on your Colab plan or out of capacity; retry later or pick another --flavor."
  exit 1
fi

echo "[orx] Uploading the source snapshot"
if ! "$colab" upload -s "$session" {archive} {remote_archive} </dev/null; then
  echo "[orx] Could not upload the source snapshot to Colab."
  exit 1
fi

driver() {{
  printf 'ORX_ARCHIVE = %s\n' "'{remote_archive_name}'"
  printf 'ORX_TIMEOUT_SECS = %s\n' {timeout}
  printf 'ORX_ENV_B64 = {{'
  for key in {env_keys}; do
    if [ -n "${{!key+x}}" ]; then
      printf "'%s': '%s', " "$key" "$(printf '%s' "${{!key}}" | base64 | tr -d '\n')"
    fi
  done
  printf '}}\n'
  cat {driver_file}
}}

code=""
driver | "$colab" exec -s "$session" --timeout {exec_timeout} 2>&1 | while IFS= read -r line || [ -n "$line" ]; do
  case "$line" in
    {sentinel}*) code="${{line#{sentinel}}}" ;;
    *) printf '%s\n' "$line" ;;
  esac
done

if [ -z "$code" ]; then
  echo "[orx] Colab did not report the run's exit status. The runtime may have been reclaimed or the connection dropped."
  exit 1
fi
exit "$code"
"#,
        cli = sh_quote(&spec.cli.to_string_lossy()),
        session = sh_quote(spec.session),
        label = spec.flavor.label().replace(['"', '$', '`', '\\'], ""),
        new_args = new_args,
        archive = sh_quote(&crate::local::bash::bash_path(spec.archive)),
        remote_archive = sh_quote(&remote_archive_name(spec.archive)),
        remote_archive_name = remote_archive_name(spec.archive),
        timeout = spec.timeout_secs,
        env_keys = env_keys,
        driver_file = sh_quote(DRIVER_FILE),
        exec_timeout = exec_timeout,
        sentinel = EXIT_SENTINEL,
    )
}

/// The static half of the kernel-side driver; the controller prepends the
/// per-run constants (`ORX_ARCHIVE`, `ORX_TIMEOUT_SECS`, `ORX_ENV_B64`).
pub fn driver_source(run_command: &str) -> String {
    use base64::Engine as _;
    let script = crate::compute::staged_script(run_command);
    let script_b64 = base64::engine::general_purpose::STANDARD.encode(script);
    DRIVER_TEMPLATE
        .replace("__ORX_SCRIPT_B64__", &script_b64)
        .replace("__ORX_SENTINEL__", EXIT_SENTINEL)
}

const DRIVER_TEMPLATE: &str = r#"
import base64 as _orx_b64
import os as _orx_os
import subprocess as _orx_sp
import sys as _orx_sys


def _orx_main():
    env = dict(_orx_os.environ)
    for key, value in ORX_ENV_B64.items():
        env[key] = _orx_b64.b64decode(value).decode("utf-8", "replace")
    env.setdefault("PYTHONUNBUFFERED", "1")
    env.setdefault("PYTHONIOENCODING", "utf-8")
    candidates = [_orx_os.path.join(base, ORX_ARCHIVE) for base in (_orx_os.getcwd(), "/content", "/")]
    archive = next((path for path in candidates if _orx_os.path.isfile(path)), None)
    if archive is None:
        print("[orx] The uploaded source snapshot is missing on the runtime.", flush=True)
        return 97
    workdir = "/content/orx-run"
    _orx_os.makedirs(_orx_os.path.join(workdir, "repo"), exist_ok=True)
    try:
        gpus = _orx_sp.run(
            ["nvidia-smi", "--query-gpu=name,memory.total", "--format=csv,noheader"],
            capture_output=True, text=True, timeout=30,
        ).stdout.strip()
        for gpu in gpus.splitlines():
            print("[orx] GPU: " + gpu, flush=True)
    except Exception:
        pass
    _orx_sp.run(["tar", "-xf", archive, "-C", _orx_os.path.join(workdir, "repo")], check=True)
    script = _orx_b64.b64decode("__ORX_SCRIPT_B64__").decode("utf-8")
    proc = _orx_sp.Popen(
        ["timeout", "--signal=TERM", "--kill-after=60", str(ORX_TIMEOUT_SECS), "bash", "-c", script],
        cwd=workdir, env=env, stdout=_orx_sp.PIPE, stderr=_orx_sp.STDOUT,
        text=True, bufsize=1, errors="replace",
    )
    for line in proc.stdout:
        _orx_sys.stdout.write(line)
        _orx_sys.stdout.flush()
    code = proc.wait()
    if code == 124:
        print("[orx] The run hit its timeout of %d seconds." % ORX_TIMEOUT_SECS, flush=True)
    return code


try:
    _orx_code = _orx_main()
except Exception as _orx_error:
    print("[orx] The Colab driver failed: %r" % (_orx_error,), flush=True)
    _orx_code = 1
print("\n__ORX_SENTINEL__%d" % _orx_code, flush=True)
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flavors_parse_with_defaults_and_high_ram() {
        let default = parse_flavor(None).unwrap();
        assert_eq!(default.id(), "t4");
        assert_eq!(default.cli_args(), ["--gpu", "T4"]);
        let a100 = parse_flavor(Some("A100:highmem")).unwrap();
        assert_eq!(a100.id(), "a100:highmem");
        assert_eq!(a100.cli_args(), ["--gpu", "A100", "--high-mem"]);
        assert_eq!(
            parse_flavor(Some("cpu")).unwrap().cli_args(),
            Vec::<String>::new()
        );
        assert_eq!(
            parse_flavor(Some("v6e1")).unwrap().cli_args(),
            ["--tpu", "v6e1"]
        );
    }

    #[test]
    fn unknown_or_unsupported_flavors_are_rejected() {
        // The CLI would silently turn these into an A100.
        assert!(parse_flavor(Some("a10g")).is_err());
        assert!(parse_flavor(Some("h100:2")).is_err());
        assert!(parse_flavor(Some("l4:highmem")).is_err());
        assert!(parse_flavor(Some("auto")).is_err());
        assert!(parse_flavor(Some("auto:-3")).is_err());
    }

    #[test]
    fn auto_picks_the_smallest_gpu_that_fits() {
        assert_eq!(parse_flavor(Some("auto:8")).unwrap().id(), "t4");
        assert_eq!(parse_flavor(Some("auto:16")).unwrap().id(), "t4");
        assert_eq!(parse_flavor(Some("auto:20")).unwrap().id(), "l4");
        assert_eq!(parse_flavor(Some("auto:30GB")).unwrap().id(), "a100");
        assert_eq!(parse_flavor(Some("auto:70")).unwrap().id(), "h100");
        assert_eq!(parse_flavor(Some("auto:90")).unwrap().id(), "g4");
        assert!(parse_flavor(Some("auto:200")).is_err());
    }

    #[test]
    fn session_names_are_short_and_safe() {
        assert_eq!(session_name("3f2a-91bc-77de-4410-aaaa"), "orx-3f2a91bc77de");
    }

    fn sample_controller() -> String {
        let flavor = parse_flavor(Some("l4")).unwrap();
        controller_script(&ControllerSpec {
            cli: Path::new("/home/me/.local/bin/colab"),
            session: "orx-abc",
            flavor: &flavor,
            archive: Path::new("/data/source-snapshots/0123456789abcdef0123.tar"),
            timeout_secs: 3600,
            env_keys: vec!["HF_TOKEN".into(), "BAD;KEY".into()],
        })
    }

    #[test]
    fn controller_assigns_uploads_runs_and_always_releases() {
        let script = sample_controller();
        let new = script.find("\"$colab\" new -s").unwrap();
        let upload = script.find("\"$colab\" upload -s").unwrap();
        let exec = script.find("\"$colab\" exec -s").unwrap();
        assert!(new < upload && upload < exec, "{script}");
        assert!(script.contains("'--gpu' 'L4'"), "{script}");
        assert!(script.contains("trap release EXIT"), "{script}");
        assert!(script.contains("--timeout 4200"), "{script}");
        assert!(
            script.contains("'orx-source-0123456789abcdef.tar'"),
            "{script}"
        );
    }

    #[test]
    fn controller_forwards_only_valid_env_names_and_never_values() {
        let script = sample_controller();
        assert!(script.contains("for key in HF_TOKEN; do"), "{script}");
        assert!(!script.contains("BAD;KEY"), "{script}");
    }

    #[test]
    fn driver_embeds_the_staged_command_and_reports_its_exit() {
        use base64::Engine as _;
        let driver = driver_source("python train.py --lr 3e-4");
        let encoded = base64::engine::general_purpose::STANDARD
            .encode(crate::compute::staged_script("python train.py --lr 3e-4"));
        assert!(driver.contains(&encoded));
        assert!(
            driver.contains("print(\"\\n__ORX_EXIT_CODE__=%d\""),
            "{driver}"
        );
        assert!(!driver.contains("__ORX_SCRIPT_B64__"));
    }
}
