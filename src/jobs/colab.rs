//! Google Colab backend — a local controller drives one Colab runtime per run
//! through the official `colab` CLI (google-colab-cli).
//!
//! The run is a local job (`colab_job`, same run-dir layout as `localbox`):
//! its `run.sh` starts `colab-controller.sh`, which
//!   1. assigns a runtime (`colab new --gpu <G>`),
//!   2. uploads the immutable source snapshot (`colab upload`),
//!   3. pipes `colab-driver.py` into `colab exec`; the cell starts a runner as
//!      its own process on the runtime and stays busy until the run ends, so
//!      the run needs nothing from this machine once it has started,
//!   4. follows the run by downloading the runner's state and numbered log
//!      chunks (`colab download`), then its `ORX_OUTPUT_DIR` as a tar,
//!   5. releases the runtime when it is done or cancelled (`colab stop`).
//!
//! When this machine sleeps, the controller just catches up. When it shuts
//! down, the controller is TERMed without a cancel and leaves the runtime up;
//! the supervisor that `orx up` starts again finds the run's controller dead,
//! sees [`RESUMABLE_FILE`], and runs the controller again, which skips to
//! following. Colab still ends a runtime after its maximum lifetime, and one
//! left idle after the run ends, whether or not this machine is back.
//!
//! Secrets never touch disk or argv: the controller reads them from its own
//! environment and pipes them, base64-encoded, into the kernel over stdin; the
//! runner gets them in its environment.
//!
//! Each run uses one of several saved Google accounts (`colab_accounts`): the
//! controller points the CLI's `HOME` at that account's folder.

use std::path::{Path, PathBuf};

use super::colab_accounts::{valid_account_id, AccountChoice};
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flavor {
    pub accelerator: Accelerator,
    pub high_mem: bool,
    pub account: AccountChoice,
}

impl Flavor {
    /// The canonical spelling, e.g. `a100:highmem@alice@gmail.com`.
    pub fn id(&self) -> String {
        let shape = if self.high_mem {
            format!("{}:highmem", self.accelerator.id)
        } else {
            self.accelerator.id.to_string()
        };
        match &self.account {
            AccountChoice::Active => shape,
            AccountChoice::Auto => format!("{shape}@auto"),
            AccountChoice::Named(name) => format!("{shape}@{name}"),
        }
    }

    /// The same machine on one named account.
    pub fn on_account(&self, id: &str) -> Self {
        Self {
            account: AccountChoice::Named(id.to_string()),
            ..self.clone()
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
/// - `auto:<GB>` for the smallest GPU with at least that much VRAM;
/// - any of these with `@<account>` (an email, or the part before `@` when it
///   is unique) to run on one saved Google account, or `@auto` for the
///   unpaused account best placed to run it. Without `@`, the default account.
///
/// Omitted means [`DEFAULT_FLAVOR`]. The `colab` CLI maps unknown GPU names to
/// an A100 silently, so anything else is rejected here.
pub fn parse_flavor(flavor: Option<&str>) -> Result<Flavor> {
    let full = flavor
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .unwrap_or(DEFAULT_FLAVOR)
        .to_ascii_lowercase();
    let (machine, account) = match full.split_once('@') {
        Some((machine, account)) => (machine.trim(), Some(account.trim())),
        None => (full.as_str(), None),
    };
    let account = match account {
        None => AccountChoice::Active,
        Some("auto") => AccountChoice::Auto,
        Some(name) if valid_account_id(name) => AccountChoice::Named(name.to_string()),
        Some(_) => return Err(anyhow!("Unknown Colab flavor '{full}'. {}", flavor_help())),
    };
    let raw = if machine.is_empty() {
        DEFAULT_FLAVOR.to_string()
    } else {
        machine.to_string()
    };
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
            account,
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
        account,
    })
}

fn flavor_help() -> String {
    let ids: Vec<_> = ACCELERATORS.iter().map(|a| a.id).collect();
    format!(
        "Use one of {}, optionally with `:highmem`, or `auto:<GB of VRAM>`; add `@<account>` or `@auto` to pick the Google account.",
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
pub const SIGN_IN_HINT: &str = super::colab_accounts::ADD_ACCOUNT_HINT;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub supported_os: bool,
    pub cli_path: Option<String>,
    pub cli_version: Option<String>,
    /// At least one Google account is saved (or about to be imported).
    pub signed_in: bool,
    pub account_count: usize,
    pub active: Option<String>,
    pub paused_count: usize,
    /// The CLI has its own sign-in in `~/.config/colab-cli` that can be imported.
    pub importable: bool,
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
    let accounts = super::colab_accounts::load_accounts();
    let importable = super::colab_accounts::cli_token().is_some();
    let signed_in = super::colab_accounts::configured() || !accounts.accounts.is_empty();
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
        account_count: accounts.accounts.len(),
        active: accounts.active.clone(),
        paused_count: accounts.accounts.iter().filter(|a| a.paused).count(),
        importable,
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
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null())
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
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

/// Colab ends a runtime after this long whatever it is doing (24 hours on Pro+).
pub const MAX_RUNTIME_SECS: u64 = 12 * 3600;

pub const CONTROLLER_FILE: &str = "colab-controller.sh";
pub const DRIVER_FILE: &str = "colab-driver.py";
/// The run's `ORX_OUTPUT_DIR` lands here in the run dir when it ends.
pub const OUTPUT_DIR: &str = "colab-output";
/// Written once the run is going on Colab without this machine; from then on
/// a dead controller is picked up again rather than failing the run.
pub const RESUMABLE_FILE: &str = "colab-resumable";
/// Written when the controller is done with the run for good.
pub const FINISHED_FILE: &str = "colab-finished";
/// How often a run was picked up again after its controller died.
const RESUMES_FILE: &str = "colab-resumes";
/// Enough for many reboots, few enough to stop a controller that dies at once.
const MAX_RESUMES: u32 = 50;
/// Where the runner keeps its log, state and outputs on the runtime, as the
/// CLI's file commands name it (relative to `/`).
const REMOTE_CTL: &str = "content/orx-run/ctl";

pub struct ControllerSpec<'a> {
    pub cli: &'a Path,
    /// The account folder the CLI uses as its home.
    pub home: &'a Path,
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

/// Whether a Colab run whose controller has died should be picked up again:
/// the run was going on Colab, nobody cancelled it, and the controller never
/// finished it. Counts the attempt, and gives up after [`MAX_RESUMES`].
pub fn take_resume(dir: &Path) -> bool {
    if !dir.join(RESUMABLE_FILE).exists()
        || dir.join(FINISHED_FILE).exists()
        || dir.join(super::localbox::CANCEL_REQUESTED_FILE).exists()
    {
        return false;
    }
    let resumes: u32 = std::fs::read_to_string(dir.join(RESUMES_FILE))
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0);
    if resumes >= MAX_RESUMES {
        return false;
    }
    std::fs::write(dir.join(RESUMES_FILE), format!("{}\n", resumes + 1)).is_ok()
}

/// The bash controller that drives one run (see the module docs). Run again
/// after it died, it skips straight to following the run.
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
    let account_env = super::colab_accounts::account_env(spec.home)
        .into_iter()
        .map(|(key, value)| format!("export {key}={}", sh_quote(&value)))
        .collect::<Vec<_>>()
        .join("\n");
    let lifetime_note = if spec.timeout_secs > MAX_RUNTIME_SECS {
        format!(
            "echo \"[orx] Colab ends a runtime after {} hours (24 on Colab Pro+), before this run's timeout. Save checkpoints as you go.\"",
            MAX_RUNTIME_SECS / 3600
        )
    } else {
        String::new()
    };
    let remote_archive = remote_archive_name(spec.archive);
    CONTROLLER_TEMPLATE
        .replace("__CLI__", &sh_quote(&spec.cli.to_string_lossy()))
        .replace("__ACCOUNT_ENV__", &account_env)
        .replace("__SESSION__", &sh_quote(spec.session))
        .replace("__CTL__", &sh_quote(REMOTE_CTL))
        .replace("__RESUMABLE__", &sh_quote(RESUMABLE_FILE))
        .replace("__FINISHED__", &sh_quote(FINISHED_FILE))
        .replace(
            "__CANCEL__",
            &sh_quote(super::localbox::CANCEL_REQUESTED_FILE),
        )
        .replace(
            "__REASON__",
            &sh_quote(super::localbox::FAILURE_REASON_FILE),
        )
        .replace("__OUTPUT_DIR__", &sh_quote(OUTPUT_DIR))
        .replace(
            "__LABEL__",
            &spec.flavor.label().replace(['"', '$', '`', '\\'], ""),
        )
        .replace("__LIFETIME_NOTE__", &lifetime_note)
        .replace("__NEW_ARGS__", &new_args)
        .replace(
            "__ARCHIVE__",
            &sh_quote(&crate::local::bash::bash_path(spec.archive)),
        )
        .replace("__REMOTE_ARCHIVE_Q__", &sh_quote(&remote_archive))
        .replace("__REMOTE_ARCHIVE__", &remote_archive)
        .replace("__TIMEOUT__", &spec.timeout_secs.to_string())
        .replace("__ENV_KEYS__", &env_keys)
        .replace("__DRIVER__", &sh_quote(DRIVER_FILE))
}

const CONTROLLER_TEMPLATE: &str = r#"#!/usr/bin/env bash
# Generated by orx: runs one experiment on a Google Colab runtime. The run
# itself goes on without this machine; this script starts it, follows its log,
# and collects its outputs. Run again after it died, it picks the run up.
set -u
colab=__CLI__
__ACCOUNT_ENV__
session=__SESSION__
ctl=__CTL__
fetch=.colab-fetch
out=__OUTPUT_DIR__
poll=${ORX_COLAB_POLL_SECS:-15}
released=0
detached=0
release() {
  if [ "$released" = 0 ]; then
    released=1
    echo "[orx] Releasing Colab runtime $session"
    "$colab" stop -s "$session" >/dev/null 2>&1 \
      || echo "[orx] Could not release $session; run: colab stop -s $session"
  fi
}
finish() {
  if [ "$detached" = 1 ]; then
    echo "[orx] This machine stopped following the run; it goes on on Colab. orx picks it up again when this machine is back."
  else
    release
    : > __FINISHED__
  fi
  rm -rf "$fetch"
}
trap finish EXIT
# A TERM without a cancel is this machine shutting down: keep the runtime.
on_term() {
  if [ -f __RESUMABLE__ ] && [ ! -f __CANCEL__ ]; then
    detached=1
  fi
  exit 143
}
trap on_term TERM INT HUP
mkdir -p "$fetch"

get() {
  "$colab" download -s "$session" "$ctl/$1" "$2" </dev/null >/dev/null 2>&1
}
field() {
  sed -n "s/^$1=//p" "$fetch/state" | head -n 1
}

if [ ! -f __RESUMABLE__ ]; then
  __LIFETIME_NOTE__
  echo "[orx] Requesting a Colab runtime: __LABEL__"
  if ! "$colab" new -s "$session" __NEW_ARGS__ </dev/null; then
    echo "[orx] Colab could not assign this runtime. The accelerator may be unavailable on your Colab plan or out of capacity; retry later or pick another --flavor."
    exit 1
  fi

  echo "[orx] Uploading the source snapshot"
  if ! "$colab" upload -s "$session" __ARCHIVE__ __REMOTE_ARCHIVE_Q__ </dev/null; then
    echo "[orx] Could not upload the source snapshot to Colab."
    exit 1
  fi

  driver() {
    printf 'ORX_ARCHIVE = %s\n' "'__REMOTE_ARCHIVE__'"
    printf 'ORX_TIMEOUT_SECS = %s\n' __TIMEOUT__
    printf 'ORX_ENV_B64 = {'
    for key in __ENV_KEYS__; do
      if [ -n "${!key+x}" ]; then
        printf "'%s': '%s', " "$key" "$(printf '%s' "${!key}" | base64 | tr -d '\n')"
      fi
    done
    printf '}\n'
    cat __DRIVER__
  }

  # The cell starts the runner and stays busy until the run ends, so Colab
  # does not count the runtime as idle. It goes on if this client goes away.
  driver | "$colab" exec -s "$session" --timeout 900 > colab-exec.log 2>&1 &
  exec_pid=$!
  waited=0
  until get state.txt "$fetch/state"; do
    if ! kill -0 "$exec_pid" 2>/dev/null && ! get state.txt "$fetch/state"; then
      echo "[orx] The run did not start on Colab:"
      tail -n 20 colab-exec.log
      exit 1
    fi
    waited=$((waited + poll))
    if [ "$waited" -ge 1200 ]; then
      echo "[orx] The run did not start on Colab within 20 minutes."
      exit 1
    fi
    sleep "$poll"
  done
  : > __RESUMABLE__
  echo "[orx] The run is going on Colab. It keeps running if this machine sleeps or shuts down; orx picks it up again when this machine is back."
else
  echo "[orx] Picking the run up again on Colab runtime $session"
fi

# Follow the run's log, a chunk file at a time, from where this machine left off.
chunk=0
offset=0
if [ -f colab-follow ]; then
  read -r chunk offset < colab-follow || true
fi
failures=0
beat=""
stale=0
while :; do
  if ! get state.txt "$fetch/state"; then
    failures=$((failures + 1))
    if [ "$failures" -ge 40 ]; then
      echo "[orx] Lost the Colab runtime: it no longer answers. Colab reclaims a runtime after its maximum lifetime (12 hours, 24 on Pro+) or when it sits idle, so a run that ended long before this machine came back loses what it left there."
      echo "Lost the Colab runtime before orx could collect the run's result." > __REASON__
      exit 1
    fi
    sleep "$poll"
    continue
  fi
  failures=0
  current=$(field chunk)
  current=${current:-0}
  finished=$(field done)
  caught_up=0
  while :; do
    name=$(printf 'log/%06d.log' "$chunk")
    get "$name" "$fetch/chunk" || break
    size=$(wc -c < "$fetch/chunk")
    if [ "$size" -gt "$offset" ]; then
      tail -c +"$((offset + 1))" "$fetch/chunk"
      offset=$size
    fi
    if [ "$chunk" -lt "$current" ]; then
      chunk=$((chunk + 1))
      offset=0
      echo "$chunk $offset" > colab-follow
      continue
    fi
    echo "$chunk $offset" > colab-follow
    caught_up=1
    break
  done
  if [ "$finished" = 1 ] && [ "$caught_up" = 1 ]; then
    break
  fi
  now=$(field beat)
  if [ "$now" = "$beat" ]; then
    stale=$((stale + 1))
    if [ "$stale" -ge 60 ]; then
      echo "[orx] The runner on Colab stopped without finishing the run."
      echo "The run stopped on Colab without reporting an exit status." > __REASON__
      exit 1
    fi
  else
    stale=0
    beat=$now
  fi
  sleep "$poll"
done

code=$(field exit)
outputs=$(field outputs)
case "$outputs" in
  parts:*)
    parts=${outputs#parts:}
    echo "[orx] Downloading the run's outputs"
    : > "$fetch/outputs.tar"
    part=0
    complete=1
    while [ "$part" -lt "$parts" ]; do
      name=$(printf 'outputs.tar.%03d' "$part")
      tries=0
      until get "$name" "$fetch/part"; do
        tries=$((tries + 1))
        if [ "$tries" -ge 5 ]; then
          complete=0
          break 2
        fi
        sleep "$poll"
      done
      cat "$fetch/part" >> "$fetch/outputs.tar"
      part=$((part + 1))
    done
    mkdir -p "$out"
    if [ "$complete" = 1 ] && tar -xf "$fetch/outputs.tar" -C "$out"; then
      echo "[orx] Saved the run's outputs in $(pwd)/$out"
    else
      echo "[orx] Could not download the run's outputs from Colab."
    fi
    ;;
  toolarge:*)
    echo "[orx] The run left ${outputs#toolarge:} bytes in ORX_OUTPUT_DIR, more than orx downloads from Colab (2 GB). Push large files to the Hugging Face Hub from the run instead."
    ;;
esac

if [ -z "$code" ]; then
  echo "[orx] Colab did not report the run's exit status."
  exit 1
fi
exit "$code"
"#;

/// The cell `colab exec` runs; the controller prepends the per-run constants
/// (`ORX_ARCHIVE`, `ORX_TIMEOUT_SECS`, `ORX_ENV_B64`). It starts the runner as
/// its own process on the runtime and stays busy until the run ends.
pub fn driver_source(run_command: &str) -> String {
    driver_source_in(run_command, "/")
}

/// [`driver_source`] for a runtime whose filesystem starts at `root`.
fn driver_source_in(run_command: &str, root: &str) -> String {
    use base64::Engine as _;
    let script = crate::compute::staged_script(run_command);
    let engine = base64::engine::general_purpose::STANDARD;
    let content = format!("{}/content", root.trim_end_matches('/'));
    let runner = RUNNER_TEMPLATE
        .replace("__ORX_SCRIPT_B64__", &engine.encode(script))
        .replace("__ORX_CONTENT__", &content);
    DRIVER_TEMPLATE
        .replace("__ORX_CONTENT__", &content)
        .replace("__ORX_RUNNER_B64__", &engine.encode(runner))
}

const DRIVER_TEMPLATE: &str = r#"
import base64 as _orx_b64
import os as _orx_os
import subprocess as _orx_sp
import sys as _orx_sys
import time as _orx_time

_ORX_CTL = "__ORX_CONTENT__/orx-run/ctl"


def _orx_state():
    try:
        with open(_ORX_CTL + "/state.txt") as f:
            return dict(line.rstrip("\n").split("=", 1) for line in f if "=" in line)
    except OSError:
        return {}


def _orx_start():
    _orx_os.makedirs(_ORX_CTL + "/log", exist_ok=True)
    if _orx_os.path.exists(_ORX_CTL + "/state.txt"):
        return None
    runner = _ORX_CTL + "/runner.py"
    with open(runner, "wb") as f:
        f.write(_orx_b64.b64decode("__ORX_RUNNER_B64__"))
    env = dict(_orx_os.environ)
    for key, value in ORX_ENV_B64.items():
        env[key] = _orx_b64.b64decode(value).decode("utf-8", "replace")
    env["ORX_ARCHIVE"] = ORX_ARCHIVE
    env["ORX_TIMEOUT_SECS"] = str(ORX_TIMEOUT_SECS)
    with open(_ORX_CTL + "/runner.err", "ab") as err:
        return _orx_sp.Popen(
            [_orx_sys.executable, runner], env=env, cwd="__ORX_CONTENT__",
            stdin=_orx_sp.DEVNULL, stdout=err, stderr=_orx_sp.STDOUT,
            start_new_session=True, close_fds=True,
        )


_orx_proc = _orx_start()
print("[orx] Runner started", flush=True)
while _orx_state().get("done") != "1":
    _orx_time.sleep(30)
    if _orx_proc is not None and _orx_proc.poll() is not None and _orx_state().get("done") != "1":
        # The runner died before it could record the run's end.
        _orx_time.sleep(5)
        if _orx_state().get("done") != "1":
            with open(_ORX_CTL + "/state.txt", "a") as f:
                f.write("exit=%d\noutputs=none\ndone=1\n" % (_orx_proc.returncode or 1))
    print(".", end="", flush=True)
"#;

const RUNNER_TEMPLATE: &str = r#"
import base64
import os
import subprocess
import sys
import threading
import time

CONTENT = "__ORX_CONTENT__"
CTL = CONTENT + "/orx-run/ctl"
WORK = CONTENT + "/orx-run"
OUT = WORK + "/outputs"
CHUNK_BYTES = 256 * 1024
PART_BYTES = 32 * 1024 * 1024
MAX_OUTPUT_BYTES = 2 * 1024 ** 3
# The GPU is billed while the runtime is up, busy or not. Sample it so an idle
# GPU shows in the log, and report how much of the run actually used it.
IDLE_WARN_SECS = 15 * 60

lock = threading.RLock()
state = {"chunk": 0, "beat": 0}


def save_state(**changes):
    with lock:
        state.update(changes)
        tmp = CTL + "/state.tmp"
        with open(tmp, "w") as f:
            for key, value in state.items():
                f.write("%s=%s\n" % (key, value))
        os.replace(tmp, CTL + "/state.txt")


class Log:
    """The run's output as numbered chunk files the controller fetches one by one."""

    def __init__(self):
        self.size = 0
        self.file = open(self.path(0), "a", encoding="utf-8", errors="replace")

    def path(self, chunk):
        return "%s/log/%06d.log" % (CTL, chunk)

    def write(self, text):
        with lock:
            self.file.write(text)
            self.file.flush()
            self.size += len(text.encode("utf-8", "replace"))
            if self.size >= CHUNK_BYTES:
                # A chunk is complete before the state names the next one.
                self.file.close()
                chunk = state["chunk"] + 1
                self.file = open(self.path(chunk), "a", encoding="utf-8", errors="replace")
                self.size = 0
                save_state(chunk=chunk)


log = Log()


def say(text):
    log.write(text + "\n")


def beat(done):
    while not done.wait(30):
        save_state(beat=state["beat"] + 1)


gpu = {"samples": 0, "busy": 0, "peak_mem_mb": 0, "total_mem_mb": 0}


def watch_gpu(done):
    idle_since = None
    warned_at = None
    while not done.wait(60):
        try:
            out = subprocess.run(
                ["nvidia-smi", "--query-gpu=utilization.gpu,memory.used,memory.total", "--format=csv,noheader,nounits"],
                capture_output=True, text=True, timeout=30,
            ).stdout.strip().splitlines()
            rows = [[float(x) for x in line.split(",")] for line in out if line.strip()]
        except Exception:
            return
        if not rows:
            return
        util = max(row[0] for row in rows)
        gpu["samples"] += 1
        gpu["busy"] += 1 if util > 0 else 0
        gpu["peak_mem_mb"] = max(gpu["peak_mem_mb"], max(row[1] for row in rows))
        gpu["total_mem_mb"] = max(row[2] for row in rows)
        now = time.monotonic()
        if util > 0:
            idle_since = warned_at = None
            continue
        idle_since = idle_since or now
        if now - idle_since >= IDLE_WARN_SECS and (warned_at is None or now - warned_at >= IDLE_WARN_SECS):
            warned_at = now
            say("[orx] The GPU has been idle for %d minutes while this run keeps spending compute units. "
                "If the GPU work is over, cancel the run; run CPU-only steps with --flavor cpu."
                % ((now - idle_since) // 60))


def gpu_summary():
    if gpu["samples"]:
        say("[orx] GPU busy in %d%% of samples; peak memory %d of %d MB."
            % (100 * gpu["busy"] // gpu["samples"], gpu["peak_mem_mb"], gpu["total_mem_mb"]))


def pack_outputs():
    """Tar ORX_OUTPUT_DIR into parts small enough to download one at a time."""
    total = 0
    for root, _, files in os.walk(OUT):
        for name in files:
            try:
                total += os.path.getsize(os.path.join(root, name))
            except OSError:
                pass
    if total == 0:
        return "none"
    if total > MAX_OUTPUT_BYTES:
        return "toolarge:%d" % total
    tar = CTL + "/outputs.tar"
    subprocess.run(["tar", "-cf", tar, "-C", OUT, "."], check=True)
    parts = 0
    with open(tar, "rb") as f:
        while True:
            data = f.read(PART_BYTES)
            if not data:
                break
            with open("%s.%03d" % (tar, parts), "wb") as part:
                part.write(data)
            parts += 1
    os.remove(tar)
    return "parts:%d" % parts


def main():
    timeout = int(os.environ["ORX_TIMEOUT_SECS"])
    name = os.environ["ORX_ARCHIVE"]
    candidates = [os.path.join(base, name) for base in (CONTENT, os.path.dirname(CONTENT), os.getcwd())]
    archive = next((path for path in candidates if os.path.isfile(path)), None)
    if archive is None:
        say("[orx] The uploaded source snapshot is missing on the runtime.")
        return 97
    os.makedirs(os.path.join(WORK, "repo"), exist_ok=True)
    os.makedirs(OUT, exist_ok=True)
    gpus = ""
    try:
        gpus = subprocess.run(
            ["nvidia-smi", "--query-gpu=name,memory.total", "--format=csv,noheader"],
            capture_output=True, text=True, timeout=30,
        ).stdout.strip()
        for line in gpus.splitlines():
            say("[orx] GPU: " + line)
    except Exception:
        pass
    subprocess.run(["tar", "-xf", archive, "-C", os.path.join(WORK, "repo")], check=True)
    script = base64.b64decode("__ORX_SCRIPT_B64__").decode("utf-8")
    env = {k: v for k, v in os.environ.items() if k not in ("ORX_ARCHIVE", "ORX_TIMEOUT_SECS")}
    env.setdefault("PYTHONUNBUFFERED", "1")
    env.setdefault("PYTHONIOENCODING", "utf-8")
    # Files written here come back to the run's colab-output folder.
    env["ORX_OUTPUT_DIR"] = OUT
    proc = subprocess.Popen(
        ["timeout", "--signal=TERM", "--kill-after=60", str(timeout), "bash", "-c", script],
        cwd=WORK, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        text=True, bufsize=1, errors="replace",
    )
    done = threading.Event()
    if gpus:
        threading.Thread(target=watch_gpu, args=(done,), daemon=True).start()
    for line in proc.stdout:
        log.write(line)
    code = proc.wait()
    done.set()
    gpu_summary()
    if code == 124:
        say("[orx] The run hit its timeout of %d seconds." % timeout)
    return code


save_state()
beating = threading.Event()
threading.Thread(target=beat, args=(beating,), daemon=True).start()
try:
    code = main()
except Exception as error:
    say("[orx] The Colab runner failed: %r" % (error,))
    code = 1
try:
    outputs = pack_outputs()
except Exception as error:
    say("[orx] Could not pack the run's outputs: %r" % (error,))
    outputs = "none"
beating.set()
save_state(exit=code, outputs=outputs, done=1)
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
        assert!(parse_flavor(Some("t4@a b")).is_err());
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
            home: Path::new("/data/colab/accounts/alice@gmail.com"),
            session: "orx-abc",
            flavor: &flavor,
            archive: Path::new("/data/source-snapshots/0123456789abcdef0123.tar"),
            timeout_secs: 3600,
            env_keys: vec!["HF_TOKEN".into(), "BAD;KEY".into()],
        })
    }

    #[test]
    fn controller_assigns_uploads_starts_and_follows() {
        let script = sample_controller();
        let new = script.find("\"$colab\" new -s").unwrap();
        let upload = script.find("\"$colab\" upload -s").unwrap();
        let exec = script.find("\"$colab\" exec -s").unwrap();
        let follow = script.find("\"$colab\" download -s").unwrap();
        assert!(new < upload && upload < exec, "{script}");
        assert!(
            follow < new,
            "the download helper is defined up front: {script}"
        );
        assert!(script.contains("'--gpu' 'L4'"), "{script}");
        assert!(script.contains("trap finish EXIT"), "{script}");
        assert!(
            script.contains("export HOME='/data/colab/accounts/alice@gmail.com'"),
            "{script}"
        );
        assert!(
            script.find("export HOME").unwrap() < script.find("trap finish EXIT").unwrap(),
            "{script}"
        );
        assert!(
            script.contains("printf 'ORX_TIMEOUT_SECS = %s\\n' 3600"),
            "{script}"
        );
        assert!(
            script.contains("'orx-source-0123456789abcdef.tar'"),
            "{script}"
        );
        assert!(
            !script.contains("__"),
            "every placeholder is filled: {script}"
        );
        assert!(!script.contains("Colab ends a runtime after"), "{script}");
    }

    #[test]
    fn controller_warns_when_the_timeout_outlives_a_colab_runtime() {
        let flavor = parse_flavor(Some("t4")).unwrap();
        let script = controller_script(&ControllerSpec {
            cli: Path::new("colab"),
            home: Path::new("/data/colab/accounts/a@b.c"),
            session: "orx-abc",
            flavor: &flavor,
            archive: Path::new("/data/source-snapshots/0123.tar"),
            timeout_secs: 20 * 3600,
            env_keys: vec![],
        });
        assert!(
            script.contains("Colab ends a runtime after 12 hours"),
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
    fn driver_embeds_the_runner_and_the_staged_command() {
        use base64::Engine as _;
        let engine = base64::engine::general_purpose::STANDARD;
        let driver = driver_source("python train.py --lr 3e-4");
        assert!(
            driver.contains("_ORX_CTL = \"/content/orx-run/ctl\""),
            "{driver}"
        );
        let start = driver.find("b64decode(\"").unwrap() + "b64decode(\"".len();
        let end = start + driver[start..].find('"').unwrap();
        let runner = String::from_utf8(engine.decode(&driver[start..end]).unwrap()).unwrap();
        assert!(runner.contains("CONTENT = \"/content\""), "{runner}");
        assert!(runner
            .contains(&engine.encode(crate::compute::staged_script("python train.py --lr 3e-4"))));
        assert!(!runner.contains("__ORX_"), "{runner}");
        assert!(!driver.contains("__ORX_"), "{driver}");
    }

    /// A scratch directory removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("orx-colab-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_dead_controller_is_picked_up_only_for_a_live_uncancelled_run() {
        let dir = TempDir::new();
        let dir = dir.path();
        assert!(!take_resume(dir), "not started on Colab yet");
        std::fs::write(dir.join(RESUMABLE_FILE), "").unwrap();
        assert!(take_resume(dir));
        std::fs::write(dir.join(super::super::localbox::CANCEL_REQUESTED_FILE), "").unwrap();
        assert!(!take_resume(dir), "cancelled");
        std::fs::remove_file(dir.join(super::super::localbox::CANCEL_REQUESTED_FILE)).unwrap();
        std::fs::write(dir.join(FINISHED_FILE), "").unwrap();
        assert!(!take_resume(dir), "finished");
        std::fs::remove_file(dir.join(FINISHED_FILE)).unwrap();
        for _ in 1..MAX_RESUMES {
            assert!(take_resume(dir));
        }
        assert!(!take_resume(dir), "gives up eventually");
    }

    /// A stand-in for the `colab` CLI: the runtime's filesystem is `$FAKE_ROOT`
    /// and `exec` runs the cell with this machine's python.
    #[cfg(target_os = "linux")]
    const FAKE_CLI: &str = r#"#!/usr/bin/env bash
cmd=$1; shift
[ "${1:-}" = -s ] && shift 2
echo "$cmd $*" >> "$FAKE_ROOT/calls"
case "$cmd" in
  new) mkdir -p "$FAKE_ROOT/content" ;;
  upload) cp "$1" "$FAKE_ROOT/$2" ;;
  download) [ -f "$FAKE_ROOT/$1" ] && cp "$FAKE_ROOT/$1" "$2" ;;
  exec) cat > "$FAKE_ROOT/cell.py"; echo $$ > "$FAKE_ROOT/exec.pid"; exec python3 "$FAKE_ROOT/cell.py" ;;
  stop) [ -f "$FAKE_ROOT/exec.pid" ] && kill "$(cat "$FAKE_ROOT/exec.pid")" 2>/dev/null; true ;;
esac
"#;

    #[cfg(target_os = "linux")]
    struct FakeRun {
        _tmp: TempDir,
        root: PathBuf,
        dir: PathBuf,
    }

    #[cfg(target_os = "linux")]
    impl FakeRun {
        fn new(command: &str) -> Self {
            use std::os::unix::fs::PermissionsExt;
            let tmp = TempDir::new();
            let root = tmp.path().join("runtime");
            let dir = tmp.path().join("run");
            std::fs::create_dir_all(&root).unwrap();
            std::fs::create_dir_all(&dir).unwrap();
            let cli = tmp.path().join("colab");
            std::fs::write(&cli, FAKE_CLI).unwrap();
            std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
            let source = tmp.path().join("src");
            std::fs::create_dir_all(&source).unwrap();
            std::fs::write(source.join("hello.txt"), "hi\n").unwrap();
            let archive = tmp.path().join("0123456789abcdef.tar");
            assert!(std::process::Command::new("tar")
                .arg("-cf")
                .arg(&archive)
                .arg("-C")
                .arg(&source)
                .arg(".")
                .status()
                .unwrap()
                .success());
            let flavor = parse_flavor(Some("t4")).unwrap();
            let home = tmp.path().join("home");
            let controller = controller_script(&ControllerSpec {
                cli: &cli,
                home: &home,
                session: "orx-test",
                flavor: &flavor,
                archive: &archive,
                timeout_secs: 600,
                env_keys: vec!["ORX_TEST_SECRET".into()],
            });
            std::fs::write(dir.join(CONTROLLER_FILE), controller).unwrap();
            std::fs::write(
                dir.join(DRIVER_FILE),
                driver_source_in(command, &root.to_string_lossy()),
            )
            .unwrap();
            Self {
                _tmp: tmp,
                root,
                dir,
            }
        }

        fn start(&self) -> std::process::Child {
            use std::os::unix::process::CommandExt;
            let log = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.dir.join("log"))
                .unwrap();
            std::process::Command::new("bash")
                .arg(CONTROLLER_FILE)
                .current_dir(&self.dir)
                .env("FAKE_ROOT", &self.root)
                .env("ORX_COLAB_POLL_SECS", "1")
                .env("ORX_TEST_SECRET", "s3cret")
                .stdout(log.try_clone().unwrap())
                .stderr(log)
                .process_group(0)
                .spawn()
                .unwrap()
        }

        fn wait(&self, mut child: std::process::Child) -> i32 {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    return status.code().unwrap_or(-1);
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "controller hung: {}",
                    self.log()
                );
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }

        fn wait_for(&self, name: &str) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            while !self.dir.join(name).exists() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "no {name}: {}",
                    self.log()
                );
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }

        fn log(&self) -> String {
            std::fs::read_to_string(self.dir.join("log")).unwrap_or_default()
        }

        fn calls(&self, cmd: &str) -> usize {
            std::fs::read_to_string(self.root.join("calls"))
                .unwrap_or_default()
                .lines()
                .filter(|line| line.split_whitespace().next() == Some(cmd))
                .count()
        }

        fn term(&self, child: &std::process::Child) {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGTERM);
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn colab_run_streams_its_whole_log_returns_outputs_and_exit_code() {
        // ~600 KB of output spans several log chunks on the runtime.
        let run = FakeRun::new(
            "cat hello.txt; echo \"secret=$ORX_TEST_SECRET\"; \
             python3 -c \"for i in range(20000): print('line %05d ' % i + 'x' * 20)\"; \
             echo done > \"$ORX_OUTPUT_DIR/result.txt\"; exit 3",
        );
        let code = run.wait(run.start());
        let log = run.log();
        assert_eq!(code, 3, "{log}");
        assert!(log.contains("hi\nsecret=s3cret\n"), "{log}");
        let lines: Vec<&str> = log.lines().filter(|l| l.starts_with("line ")).collect();
        assert_eq!(lines.len(), 20000, "every line exactly once");
        assert!(lines.windows(2).all(|w| w[0] < w[1]), "in order");
        assert!(
            run.root.join("content/orx-run/ctl/log/000002.log").exists(),
            "the log rotated"
        );
        assert_eq!(
            std::fs::read_to_string(run.dir.join(OUTPUT_DIR).join("result.txt")).unwrap(),
            "done\n"
        );
        assert!(run.dir.join(FINISHED_FILE).exists());
        assert_eq!(run.calls("stop"), 1, "released");
        assert!(!run.dir.join(".colab-fetch").exists());
        let driver = std::fs::read_to_string(run.dir.join(DRIVER_FILE)).unwrap();
        assert!(!driver.contains("s3cret"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_shutdown_leaves_the_run_going_and_a_restart_picks_it_up() {
        let run = FakeRun::new("echo before; sleep 4; echo after");
        let child = run.start();
        run.wait_for(RESUMABLE_FILE);
        run.term(&child);
        assert_eq!(run.wait(child), 143);
        assert_eq!(run.calls("stop"), 0, "kept the runtime: {}", run.log());
        assert!(!run.dir.join(FINISHED_FILE).exists());
        assert!(take_resume(&run.dir));

        let code = run.wait(run.start());
        let log = run.log();
        assert_eq!(code, 0, "{log}");
        assert!(log.contains("Picking the run up again"), "{log}");
        assert_eq!(log.matches("before\n").count(), 1, "{log}");
        assert_eq!(log.matches("after\n").count(), 1, "{log}");
        assert_eq!(run.calls("new"), 1);
        assert_eq!(run.calls("stop"), 1);
        assert!(!take_resume(&run.dir));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_cancel_releases_the_runtime() {
        let run = FakeRun::new("sleep 30");
        let child = run.start();
        run.wait_for(RESUMABLE_FILE);
        std::fs::write(
            run.dir.join(super::super::localbox::CANCEL_REQUESTED_FILE),
            "",
        )
        .unwrap();
        run.term(&child);
        assert_eq!(run.wait(child), 143);
        assert_eq!(run.calls("stop"), 1, "{}", run.log());
        assert!(run.dir.join(FINISHED_FILE).exists());
        assert!(!take_resume(&run.dir));
    }
}
