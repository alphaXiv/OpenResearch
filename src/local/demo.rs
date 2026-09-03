//! The first-run nanochat project: embedded source, deterministic local Git
//! history, and three curated harness-native conversations.

use std::path::{Path, PathBuf};
use std::process::Command;

use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{anyhow, Result};
use crate::store::{Store, StoredChatMessage, StoredChatSession, StoredRun};

use super::chat::{WirePart, WireToolState};
use super::model::{LocalExperiment, LocalProject};

pub const PROJECT_ID: &str = "demo_nanochat_v1";
pub const PROJECT_SLUG: &str = "nanochat";
const FALLBACK_PROJECT_SLUG: &str = "demo_nanochat_v1";
const EXPERIMENT_ID: &str = "demo_nanochat_cpu_v1";
const RUN_ID: &str = "demo_nanochat_run_v1";
const SESSION_ID: &str = "chat_demo_nanochat_v1";
const USER_MESSAGE_ID: &str = "msg_demo_nanochat_user_v1";
const ASSISTANT_MESSAGE_ID: &str = "msg_demo_nanochat_assistant_v1";
const FIGURE_SESSION_ID: &str = "chat_demo_nanochat_figures_v1";
const FIGURE_USER_MESSAGE_ID: &str = "msg_demo_nanochat_figures_user_v1";
const FIGURE_ASSISTANT_MESSAGE_ID: &str = "msg_demo_nanochat_figures_assistant_v1";
const LITERATURE_SESSION_ID: &str = "chat_demo_nanochat_literature_v1";
const LITERATURE_USER_MESSAGE_ID: &str = "msg_demo_nanochat_literature_user_v1";
const LITERATURE_ASSISTANT_MESSAGE_ID: &str = "msg_demo_nanochat_literature_assistant_v1";
const OWNER: &str = "openresearch-demo";
const REPO: &str = "nanochat";
const BRANCH: &str = "orx/cpu-apple-silicon-end-to-end-baseline";
const LR_PROBE_EXPERIMENT_ID: &str = "demo_nanochat_lr_probe_v1";
const LR_PROBE_BRANCH: &str = "orx/matrix-lr-2x-probe";
const VOCAB_PROBE_EXPERIMENT_ID: &str = "demo_nanochat_vocab_probe_v1";
const VOCAB_PROBE_BRANCH: &str = "orx/vocab-8192-probe";
// Same environment and data setup as runs/runcpu.sh, then a 200-step base-training probe.
const PROBE_SETUP: &str = "export NANOCHAT_BASE_DIR=\"$PWD/.cache/nanochat\" UV_CACHE_DIR=\"$PWD/.cache/uv\" && mkdir -p \"$NANOCHAT_BASE_DIR\" \"$UV_CACHE_DIR\" && (command -v uv >/dev/null || curl -LsSf https://astral.sh/uv/install.sh | sh) && ([ -d .venv ] || uv venv) && uv sync --extra cpu && source .venv/bin/activate && python -m nanochat.dataset -n 8";
// --warmdown-ratio=0 keeps the LR schedule identical to the baseline's first 200 steps.
const PROBE_TRAIN: &str = "python -m scripts.base_train --depth=6 --head-dim=64 --window-pattern=L --max-seq-len=512 --device-batch-size=32 --total-batch-size=16384 --eval-every=50 --eval-tokens=524288 --core-metric-every=-1 --sample-every=-1 --num-iterations=200 --warmdown-ratio=0";
const PROBE_TOK_TRAIN: &str = "python -m scripts.tok_train --max-chars=2000000000";
const BASELINE_SHA: &str = "96098ad3f3708748f693c28194520ae13afb9c69";
const EXPERIMENT_SHA: &str = "b302007b336e47028e321b0d920f030445c4db67";

const TURN_CONTEXT: &str = r#"<openresearch-demo-evidence>
This is a recorded OpenResearch demo run. The project's Artifacts/evidence directory contains real checkpoint metadata, the trained tokenizer, structured training and evaluation metrics, the final inference transcript, and run-manifest.json. To reduce the bundled demo project's download size, the multi-gigabyte model checkpoints, optimizer states, datasets, and environment are intentionally not included; the manifest records their original paths, sizes, hashes, and omission status. Do not search for or claim access to omitted files. Before proposing work that requires model weights, explain that the weights must be regenerated or downloaded. When the user asks you to choose an autonomous follow-up, prefer an analysis supported by the bundled evidence unless they explicitly ask to regenerate or download the weights.
</openresearch-demo-evidence>"#;

const USER_PROMPT: &str = "Run nanochat's CPU/Apple-Silicon pipeline end-to-end with bash runs/runcpu.sh (the local shrunk-down version, not speedrun.sh, ~40 min), streaming output and surfacing val_bpb/eval numbers as they appear, fixing any setup errors in place, and when it finishes chat with the model via python -m scripts.chat_cli -p \"What is the capital of France?\" to confirm it works.";

const FIGURE_USER_PROMPT: &str = "Using nanochat’s recorded base-training and SFT logs, create publication-quality SVG-only figures for base and SFT loss/validation BPB, Apple-Silicon throughput, and CORE evaluation results. Use Helvetica Neue, a restrained academic style, exact metric labels, colorblind-safe colors, and no decorative infographics or PNG duplicates.";

const LITERATURE_USER_PROMPT: &str = "My 73.5M-parameter nanochat model was pretrained on 81.9M tokens and then SFT-tuned on SmolTalk, MMLU, and GSM8K. SFT validation loss improved substantially, but generation remains repetitive and benchmark performance is weak. Based on the literature, is the main bottleneck insufficient pretraining, model size, data quality, or the SFT recipe—and what experiment should I run next?";

const FIGURE_BOOTSTRAP_CONTEXT: &str = r#"You are continuing a live OpenResearch demo session. The user asked you to turn nanochat's recorded base-training and SFT logs into four publication-quality SVG-only figures using Helvetica Neue, exact metric labels, colorblind-safe colors, and a restrained academic style.

You inspected the complete recorded logs, parsed all 5,000 base-training loss points, all 1,499 SFT loss points, validation BPB checkpoints, phase throughput, and CORE evaluation metrics. You generated and visually checked these four durable artifacts: nanochat-base-training-curves.svg, nanochat-sft-training-curves.svg, nanochat-training-throughput.svg, and nanochat-core-evaluation.svg. No PNG duplicates or decorative infographics were retained.

Continue naturally from this completed state. The figures already exist in the project's Artifacts area; do not claim to regenerate them unless the user asks you to."#;

const LITERATURE_BOOTSTRAP_CONTEXT: &str = r#"You are continuing a live OpenResearch demo session. The user asked whether weak, repetitive generations from the completed 73.5M-parameter nanochat run were primarily caused by insufficient pretraining, model size, data quality, or the SFT recipe.

You inspected the recorded training evidence and relevant scaling, alignment, and small-model literature. You concluded that insufficient pretraining is the primary bottleneck, model size is a secondary ceiling, and SFT likely amplifies repetition without being able to create knowledge absent from pretraining. The model saw 81.92M tokens, or 3.53 tokens per nanochat scaling parameter, and base validation BPB was still improving at step 5,000.

You recommended a one-factor pretraining-token ablation: keep the d6 model and every other recipe choice fixed, train from scratch to nanochat's target ratio of 12 (278,396,928 tokens / 16,992 steps), evaluate checkpoints at 5,000, about 11,000, and 16,992 steps, then apply the identical 1,500-step SFT recipe only to the final checkpoint. The full note is saved as nanochat-bottleneck-diagnosis.md.

Continue naturally from this completed state."#;

const BOOTSTRAP_CONTEXT: &str = r#"You are continuing a live OpenResearch demo session. The following exchange already happened and is authoritative project context.

The user asked you to run nanochat's CPU/Apple-Silicon pipeline end to end, surface validation and evaluation metrics, repair setup issues, and confirm the trained model can answer the capital of France.

You inspected the repository and prepared the CPU pipeline before launching it: caches were made checkout-local, SFT conversations longer than a row were excluded, all-masked SFT batches were guarded, periodic ChatCORE was disabled for Apple-Silicon memory safety, and the chat CLI was made to re-enter the project environment. You created the CPU baseline experiment and completed one successful run.

Recorded results: a 6-layer 73.5M-parameter model trained for 5,000 base steps over 81.92M tokens; final training validation BPB 1.165758; base-eval train/validation BPB 1.152185/1.119301; SFT completed 1,500 steps with final validation BPB 0.7389. The final CLI loaded the SFT checkpoint and answered that the capital of France is Paris. The results are saved in cpu-apple-silicon-pipeline-results.md.

Two idle follow-up experiments already branch from that baseline, each with a complete run command and no code changes needed: demo_nanochat_lr_probe_v1 (Muon matrix LR 2× probe) and demo_nanochat_vocab_probe_v1 (8,192-token vocabulary probe). Both are short 200-step base-training probes on the local backend whose learning-rate schedule matches the baseline's first 200 steps, so their validation BPB compares directly with the baseline's 1.940739 at step 100 and 1.762539 at step 200.

Operational note: when the user asks to run one of them, launch it immediately with `orx exp run <expId>`. Do not load skills, read compute references, or re-inspect the experiment tree first; follow the run's logs and report the comparison.

Continue naturally from this completed state. Do not claim you are rerunning the historical training unless the user asks you to."#;

const RESULT_MARKDOWN: &str = r#"Completed the nanochat CPU / Apple-Silicon pipeline end to end.

- Base training: 5,000 steps, 81.92M tokens, final validation BPB 1.165758
- Base evaluation: train BPB 1.152185, validation BPB 1.119301
- SFT: 1,500 steps, final validation BPB 0.7389
- Chat confirmation: the model answered that the capital of France is Paris
- Evidence pack: [README](evidence/README.md), structured metrics, checkpoint metadata, tokenizer, inference transcript, and run manifest
"#;

const REPORT: &str = r#"# nanochat CPU / Apple-Silicon pipeline results

This bundled demo records a completed local `runs/runcpu.sh` pipeline on Apple Silicon. It is historical evidence included with OpenResearch; onboarding does not rerun training on the new user's machine.

## Base training and evaluation

- Model: d6, 6 layers, 73.5M parameters, sequence length 512
- Training: 5,000 steps, 81.92M tokens, 131.55 minutes
- Final training-loop validation BPB: 1.165758
- Base-eval train BPB: 1.152185
- Base-eval validation BPB: 1.119301
- CORE accuracy: wikidata 0.0000, openbook 0.2500, winogrande 0.5625, operators 0.0000

## Supervised fine-tuning

- Started from the validated base checkpoint with a fresh optimizer
- Training: 1,500 steps, 39.07 minutes
- Validation BPB: 1.0174 → 1.0580 → 0.9914 → 0.9513 → 0.9141 → 0.8483 → 0.7950 → 0.7486 → 0.7389
- Final/minimum validation BPB: 0.7389

## Chat confirmation

The final command loaded the step-1499 SFT checkpoint on MPS and answered: “Paris … The capital of France is Paris.”

## Evidence pack

The `evidence/` directory contains the real checkpoint metadata, trained tokenizer, structured metrics, final inference transcript, and a manifest of both bundled and omitted run outputs. To reduce the bundled demo project's download size, large model weights, optimizer states, datasets, and environments are intentionally omitted.

## Portable setup repairs

- Kept nanochat and uv caches inside the checkout.
- Excluded conversations longer than the SFT row capacity and guarded all-masked batches.
- Disabled memory-heavy periodic ChatCORE while retaining validation BPB.
- Made the chat CLI reuse the project environment and recorded cache location.
"#;

const RUN_LOG: &str = include_str!("../../demo/nanochat/run-output.txt");

#[derive(RustEmbed)]
#[folder = "demo/nanochat/base/"]
struct BaseAssets;

#[derive(RustEmbed)]
#[folder = "demo/nanochat/experiment/"]
struct ExperimentAssets;

#[derive(RustEmbed)]
#[folder = "demo/nanochat/figures/"]
struct FigureAssets;

#[derive(RustEmbed)]
#[folder = "demo/nanochat/evidence/"]
struct EvidenceAssets;

const BOTTLENECK_REPORT: &str =
    include_str!("../../demo/nanochat/reports/nanochat-bottleneck-diagnosis.md");

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DemoSelection {
    pub harness: String,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    pub reasoning_level: Option<String>,
}

#[derive(Debug)]
pub struct DemoCompletion {
    pub project: LocalProject,
    pub selection: DemoSelection,
    pub newly_created: bool,
}

/// Install or return the first-run demo under production data/cache roots.
pub fn complete_onboarding(selection: DemoSelection) -> Result<DemoCompletion> {
    let store = Store::open()?;
    let data_root = crate::store::data_dir();
    let repo = super::git::clone_path(OWNER, REPO);
    seed_at(&store, &data_root, &repo, selection)
}

pub(crate) fn installed_origin(owner: &str, repo: &str) -> Option<PathBuf> {
    if owner != OWNER || repo != REPO {
        return None;
    }
    Store::open().ok()?.get_local_project(PROJECT_ID).ok()??;
    let origin = crate::store::data_dir().join("demo-repos/nanochat.git");
    origin.exists().then_some(origin)
}

pub(crate) fn turn_context(project_id: &str) -> Option<&'static str> {
    (project_id == PROJECT_ID).then_some(TURN_CONTEXT)
}

pub(crate) fn session_start_ref(owner: &str, repo: &str, session_id: &str) -> Option<&'static str> {
    (owner == OWNER
        && repo == REPO
        && matches!(
            session_id,
            SESSION_ID | FIGURE_SESSION_ID | LITERATURE_SESSION_ID
        ))
    .then_some(EXPERIMENT_SHA)
}

/// Repoint the embedded demo's local origin after the data directory moves.
pub fn repair_installed_origin(data_root: &Path) -> Result<()> {
    repair_installed_origin_at(data_root, &super::git::clone_path(OWNER, REPO))
}

fn repair_installed_origin_at(data_root: &Path, repo: &Path) -> Result<()> {
    let store = Store::open_at(data_root.to_path_buf())?;
    let Some(project) = store.get_local_project(PROJECT_ID)? else {
        return Ok(());
    };
    if project.repo_path != repo.to_string_lossy() {
        return Err(anyhow!(
            "the installed nanochat demo repository is not at its reserved cache path"
        ));
    }
    if !repo.join(".git").is_dir() {
        return Ok(());
    }
    let bare = data_root.join("demo-repos/nanochat.git");
    if !matches!(
        git(&bare, &["rev-parse", "--is-bare-repository"]).as_deref(),
        Ok("true")
    ) {
        return Err(anyhow!(
            "the moved nanochat demo origin at {} is not a bare Git repository",
            bare.display()
        ));
    }
    let origin = bare.to_string_lossy();
    if git(repo, &["remote", "get-url", "origin"]).is_ok() {
        git(repo, &["remote", "set-url", "origin", origin.as_ref()])?;
    } else {
        git(repo, &["remote", "add", "origin", origin.as_ref()])?;
    }
    Ok(())
}

fn seed_at(
    store: &Store,
    data_root: &Path,
    repo: &Path,
    selection: DemoSelection,
) -> Result<DemoCompletion> {
    let bare = data_root.join("demo-repos").join("nanochat.git");
    let commit_sha = install_repository(repo, &bare)?;

    let project_slug = demo_project_slug(store)?;
    let files = data_root.join("files").join(&project_slug);
    std::fs::create_dir_all(&files)?;
    std::fs::write(files.join("cpu-apple-silicon-pipeline-results.md"), REPORT)?;
    std::fs::write(
        files.join("nanochat-bottleneck-diagnosis.md"),
        BOTTLENECK_REPORT,
    )?;
    for name in FigureAssets::iter() {
        let asset = FigureAssets::get(name.as_ref())
            .ok_or_else(|| anyhow!("embedded demo figure is missing: {name}"))?;
        std::fs::write(files.join(name.as_ref()), asset.data.as_ref())?;
    }
    write_assets::<EvidenceAssets>(&files.join("evidence"))?;
    let logs = data_root.join("run-logs");
    std::fs::create_dir_all(&logs)?;
    std::fs::write(logs.join(format!("{RUN_ID}.log")), RUN_LOG)?;

    let project = LocalProject {
        id: PROJECT_ID.into(),
        name: "nanochat (demo)".into(),
        slug: project_slug,
        github_owner: OWNER.into(),
        github_repo: REPO.into(),
        github_sync_enabled: false,
        baseline_branch: "main".into(),
        repo_path: repo.to_string_lossy().into_owned(),
        run_command: Some("bash runs/runcpu.sh".into()),
        paper_id: None,
        created_at: 1_785_812_413_316,
        updated_at: 1_785_879_263_859,
    };
    let experiment = LocalExperiment {
        id: EXPERIMENT_ID.into(),
        project_id: PROJECT_ID.into(),
        parent_experiment_id: None,
        slug: "cpu-apple-silicon-end-to-end-baseline".into(),
        branch_name: BRANCH.into(),
        title: Some("CPU Apple-Silicon end-to-end baseline".into()),
        description: Some(
            "Completed a portable CPU/MPS pipeline: 5,000 base steps, base evaluation, 1,500 SFT steps, and a successful Paris chat confirmation. Final base validation BPB was 1.165758 and final SFT validation BPB was 0.7389."
                .into(),
        ),
        run_command: project.run_command.clone().unwrap_or_default(),
        agent_status: "idle".into(),
        created_at: 1_785_824_322_614,
        updated_at: 1_785_879_252_272,
        chat_session_id: Some(SESSION_ID.into()),
    };
    let lr_probe = LocalExperiment {
            id: LR_PROBE_EXPERIMENT_ID.into(),
            project_id: PROJECT_ID.into(),
            parent_experiment_id: Some(EXPERIMENT_ID.into()),
            slug: "matrix-lr-2x-probe".into(),
            branch_name: LR_PROBE_BRANCH.into(),
            title: Some("Muon matrix LR 2× probe (200 steps)".into()),
            description: Some(
                "A lightweight early-training probe: the baseline d6 recipe with --matrix-lr raised from 0.02 to 0.04, trained for 200 steps with validation every 50 and no learning-rate warmdown, so the learning-rate schedule matches the baseline's first 200 steps. Compare against the baseline curve, which reached val_bpb 1.940739 at step 100 and 1.762539 at step 200. Skips base_eval and SFT: a few minutes on Apple Silicon, longer on plain CPU."
                    .into(),
            ),
            run_command: format!(
                "{PROBE_SETUP} && {PROBE_TOK_TRAIN} && {PROBE_TRAIN} --matrix-lr=0.04"
            ),
            agent_status: "idle".into(),
            created_at: 1_785_879_300_000,
            updated_at: 1_785_879_300_000,
            chat_session_id: None,
        };
    let vocab_probe = LocalExperiment {
            id: VOCAB_PROBE_EXPERIMENT_ID.into(),
            project_id: PROJECT_ID.into(),
            parent_experiment_id: Some(EXPERIMENT_ID.into()),
            slug: "vocab-8192-probe".into(),
            branch_name: VOCAB_PROBE_BRANCH.into(),
            title: Some("8,192-token vocabulary probe (200 steps)".into()),
            description: Some(
                "A lightweight tokenizer probe: retrain the BPE tokenizer with an 8,192-token vocabulary instead of 32,768, then run the baseline d6 recipe for 200 steps with validation every 50 and no learning-rate warmdown, so the learning-rate schedule matches the baseline's first 200 steps. Bits per byte stays comparable across vocabularies, so compare directly against the baseline's 1.940739 at step 100 and 1.762539 at step 200. Skips base_eval and SFT: a few minutes on Apple Silicon, longer on plain CPU."
                    .into(),
            ),
            run_command: format!(
                "{PROBE_SETUP} && {PROBE_TOK_TRAIN} --vocab-size=8192 && {PROBE_TRAIN}"
            ),
            agent_status: "idle".into(),
            created_at: 1_785_879_360_000,
            updated_at: 1_785_879_360_000,
            chat_session_id: None,
        };
    let run = StoredRun {
        id: RUN_ID.into(),
        experiment_id: EXPERIMENT_ID.into(),
        project_id: PROJECT_ID.into(),
        status: "done".into(),
        backend_json: json!({ "kind": "local_job", "jobId": "demo:nanochat" }).to_string(),
        command: project.run_command.clone().unwrap_or_default(),
        created_at: 1_785_865_810_129,
        updated_at: 1_785_879_208_664,
        ended_at: Some(1_785_879_208_664),
        exit_code: Some(0),
        commit_sha: Some(commit_sha.clone()),
        result_markdown: Some(RESULT_MARKDOWN.into()),
        cancel_requested: false,
        chat_session_id: None,
    };
    let session = StoredChatSession {
        id: SESSION_ID.into(),
        project_id: PROJECT_ID.into(),
        harness: selection.harness.clone(),
        native_session_id: None,
        title: Some("Run Nanochat CPU Pipeline End-to-End".into()),
        title_source: Some("generated".into()),
        model: selection.model.clone(),
        service_tier: None,
        permission_mode: selection.permission_mode.clone(),
        plan_mode: false,
        plan_reset_pending: false,
        reasoning_level: selection.reasoning_level.clone(),
        archived: false,
        context_usage_json: None,
        bootstrap_context: Some(BOOTSTRAP_CONTEXT.into()),
        active_leaf_id: Some(ASSISTANT_MESSAGE_ID.into()),
        parent_session_id: None,
        created_at: 1_785_824_322_614,
        updated_at: 1_785_879_263_859,
    };
    let user = StoredChatMessage {
        id: USER_MESSAGE_ID.into(),
        session_id: SESSION_ID.into(),
        role: "user".into(),
        parts_json: serde_json::to_string(&vec![WirePart::text("user-prompt", USER_PROMPT)])?,
        created_at: 1_785_824_322_627,
        parent_id: None,
        base_native_session_id: None,
        result_native_session_id: None,
    };
    let assistant = StoredChatMessage {
        id: ASSISTANT_MESSAGE_ID.into(),
        session_id: SESSION_ID.into(),
        role: "assistant".into(),
        parts_json: serde_json::to_string(&assistant_parts(&selection.harness))?,
        created_at: 1_785_824_322_629,
        parent_id: Some(USER_MESSAGE_ID.into()),
        base_native_session_id: None,
        result_native_session_id: None,
    };
    let figure_session = StoredChatSession {
        id: FIGURE_SESSION_ID.into(),
        project_id: PROJECT_ID.into(),
        harness: selection.harness.clone(),
        native_session_id: None,
        title: Some("Visualize Nanochat Training Results".into()),
        title_source: Some("generated".into()),
        model: selection.model.clone(),
        service_tier: None,
        permission_mode: selection.permission_mode.clone(),
        plan_mode: false,
        plan_reset_pending: false,
        reasoning_level: selection.reasoning_level.clone(),
        archived: false,
        context_usage_json: None,
        bootstrap_context: Some(FIGURE_BOOTSTRAP_CONTEXT.into()),
        active_leaf_id: Some(FIGURE_ASSISTANT_MESSAGE_ID.into()),
        parent_session_id: None,
        created_at: 1_785_824_322_630,
        updated_at: 1_785_879_263_858,
    };
    let figure_user = StoredChatMessage {
        id: FIGURE_USER_MESSAGE_ID.into(),
        session_id: FIGURE_SESSION_ID.into(),
        role: "user".into(),
        parts_json: serde_json::to_string(&vec![WirePart::text(
            "figure-user-prompt",
            FIGURE_USER_PROMPT,
        )])?,
        created_at: 1_785_824_322_631,
        parent_id: None,
        base_native_session_id: None,
        result_native_session_id: None,
    };
    let figure_assistant = StoredChatMessage {
        id: FIGURE_ASSISTANT_MESSAGE_ID.into(),
        session_id: FIGURE_SESSION_ID.into(),
        role: "assistant".into(),
        parts_json: serde_json::to_string(&figure_assistant_parts(&selection.harness))?,
        created_at: 1_785_824_322_633,
        parent_id: Some(FIGURE_USER_MESSAGE_ID.into()),
        base_native_session_id: None,
        result_native_session_id: None,
    };
    let literature_session = StoredChatSession {
        id: LITERATURE_SESSION_ID.into(),
        project_id: PROJECT_ID.into(),
        harness: selection.harness.clone(),
        native_session_id: None,
        title: Some("Diagnosing Nanochat Training Bottlenecks".into()),
        title_source: Some("generated".into()),
        model: selection.model,
        service_tier: None,
        permission_mode: selection.permission_mode,
        plan_mode: false,
        plan_reset_pending: false,
        reasoning_level: selection.reasoning_level,
        archived: false,
        context_usage_json: None,
        bootstrap_context: Some(LITERATURE_BOOTSTRAP_CONTEXT.into()),
        active_leaf_id: Some(LITERATURE_ASSISTANT_MESSAGE_ID.into()),
        parent_session_id: None,
        created_at: 1_785_824_322_634,
        updated_at: 1_785_879_263_857,
    };
    let literature_user = StoredChatMessage {
        id: LITERATURE_USER_MESSAGE_ID.into(),
        session_id: LITERATURE_SESSION_ID.into(),
        role: "user".into(),
        parts_json: serde_json::to_string(&vec![WirePart::text(
            "literature-user-prompt",
            LITERATURE_USER_PROMPT,
        )])?,
        created_at: 1_785_824_322_635,
        parent_id: None,
        base_native_session_id: None,
        result_native_session_id: None,
    };
    let literature_assistant = StoredChatMessage {
        id: LITERATURE_ASSISTANT_MESSAGE_ID.into(),
        session_id: LITERATURE_SESSION_ID.into(),
        role: "assistant".into(),
        parts_json: serde_json::to_string(&literature_assistant_parts(&selection.harness))?,
        created_at: 1_785_824_322_636,
        parent_id: Some(LITERATURE_USER_MESSAGE_ID.into()),
        base_native_session_id: None,
        result_native_session_id: None,
    };
    let cache_root = repo
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or_else(|| anyhow!("demo repository is not under the reserved cache layout"))?;
    let worktree = cache_root
        .join("worktrees")
        .join(PROJECT_ID)
        .join(SESSION_ID);
    super::git::ensure_worktree_at(repo, &worktree, &commit_sha)?;
    let figure_worktree = cache_root
        .join("worktrees")
        .join(PROJECT_ID)
        .join(FIGURE_SESSION_ID);
    super::git::ensure_worktree_at(repo, &figure_worktree, &commit_sha)?;
    let literature_worktree = cache_root
        .join("worktrees")
        .join(PROJECT_ID)
        .join(LITERATURE_SESSION_ID);
    super::git::ensure_worktree_at(repo, &literature_worktree, &commit_sha)?;
    let newly_created = store.create_demo_snapshot(
        &project,
        &[experiment, lr_probe, vocab_probe],
        &run,
        &[session, figure_session, literature_session],
        &[
            user,
            assistant,
            figure_user,
            figure_assistant,
            literature_user,
            literature_assistant,
        ],
    )?;
    validate_snapshot(store, repo, newly_created)
}

fn demo_project_slug(store: &Store) -> Result<String> {
    if let Some(project) = store.get_local_project(PROJECT_ID)? {
        return Ok(project.slug);
    }
    if store.get_local_project_by_slug(PROJECT_SLUG)?.is_none() {
        return Ok(PROJECT_SLUG.into());
    }
    let mut candidate = FALLBACK_PROJECT_SLUG.to_string();
    let mut suffix = 2;
    while store.get_local_project_by_slug(&candidate)?.is_some() {
        candidate = format!("{FALLBACK_PROJECT_SLUG}_{suffix}");
        suffix += 1;
    }
    Ok(candidate)
}

fn validate_snapshot(store: &Store, repo: &Path, newly_created: bool) -> Result<DemoCompletion> {
    let project = store
        .get_local_project(PROJECT_ID)?
        .ok_or_else(|| anyhow!("demo project seed did not persist"))?;
    if project.repo_path != repo.to_string_lossy()
        || project.github_owner != OWNER
        || project.github_repo != REPO
        || project.baseline_branch != "main"
    {
        return Err(anyhow!(
            "the reserved demo project already exists with unexpected repository metadata; delete it and retry onboarding"
        ));
    }
    let experiments = store.list_experiments_by_project(PROJECT_ID)?;
    let runs = store.list_runs_by_project(PROJECT_ID)?;
    let sessions = store.list_chat_sessions_by_project(PROJECT_ID)?;
    let expected_experiments = [
        (EXPERIMENT_ID, BRANCH),
        (LR_PROBE_EXPERIMENT_ID, LR_PROBE_BRANCH),
        (VOCAB_PROBE_EXPERIMENT_ID, VOCAB_PROBE_BRANCH),
    ];
    let experiments_match = experiments.len() == expected_experiments.len()
        && experiments
            .iter()
            .zip(expected_experiments)
            .all(|(actual, (id, branch))| actual.id == id && actual.branch_name == branch);
    if !experiments_match
        || runs.len() != 1
        || runs[0].id != RUN_ID
        || runs[0].status != "done"
        || runs[0].exit_code != Some(0)
        || runs[0].commit_sha.as_deref() != Some(EXPERIMENT_SHA)
        || sessions.len() != 3
        || sessions[0].id != SESSION_ID
        || sessions[1].id != FIGURE_SESSION_ID
        || sessions[2].id != LITERATURE_SESSION_ID
    {
        return Err(anyhow!(
            "the reserved demo project is incomplete or modified; delete it and retry onboarding"
        ));
    }
    let messages = store.list_chat_messages(SESSION_ID)?;
    if messages.len() != 2
        || messages[0].id != USER_MESSAGE_ID
        || messages[0].role != "user"
        || messages[1].id != ASSISTANT_MESSAGE_ID
        || messages[1].role != "assistant"
    {
        return Err(anyhow!(
            "the reserved demo conversation is incomplete or modified; delete it and retry onboarding"
        ));
    }
    let figure_messages = store.list_chat_messages(FIGURE_SESSION_ID)?;
    if figure_messages.len() != 2
        || figure_messages[0].id != FIGURE_USER_MESSAGE_ID
        || figure_messages[0].role != "user"
        || figure_messages[1].id != FIGURE_ASSISTANT_MESSAGE_ID
        || figure_messages[1].role != "assistant"
    {
        return Err(anyhow!(
            "the reserved demo figure conversation is incomplete or modified; delete it and retry onboarding"
        ));
    }
    let literature_messages = store.list_chat_messages(LITERATURE_SESSION_ID)?;
    if literature_messages.len() != 2
        || literature_messages[0].id != LITERATURE_USER_MESSAGE_ID
        || literature_messages[0].role != "user"
        || literature_messages[1].id != LITERATURE_ASSISTANT_MESSAGE_ID
        || literature_messages[1].role != "assistant"
    {
        return Err(anyhow!(
            "the reserved demo literature conversation is incomplete or modified; delete it and retry onboarding"
        ));
    }
    let stored = sessions
        .iter()
        .find(|session| session.id == SESSION_ID)
        .ok_or_else(|| anyhow!("the reserved demo session is missing"))?;
    Ok(DemoCompletion {
        project,
        newly_created,
        selection: DemoSelection {
            harness: stored.harness.clone(),
            model: stored.model.clone(),
            permission_mode: stored.permission_mode.clone(),
            reasoning_level: stored.reasoning_level.clone(),
        },
    })
}

const BASE_PROGRESS: &[&str] = &[
    "Base training is under way. Initial validation BPB was **3.195800**; by step 100 it had already dropped to **1.940739**, and MPS throughput is steady at roughly 10–11k tokens/s.",
    "Halfway through base training: **step 2,500 `val_bpb = 1.248040`**. Every one of the 25 validation points so far has improved on the last, and the remaining base ETA is about 67 minutes.",
    "Base training finished: **final step 5,000 `val_bpb = 1.165758`**, down from 3.195800 at initialization with no regressions across all 50 validation points. The base phase took 131.55 minutes on this Mac and the pipeline is moving into `base_eval`.",
];

const SFT_PROGRESS: &[&str] = &[
    "The memory-safe SFT run is healthy: step 100 loss is **3.2589**, down from 6.17 during initialization, with no skipped-loop or `NaN` recurrence, and the run is past step 200 without invoking the memory-heavy ChatCORE path.",
    "SFT is converging cleanly. Validation BPB after an early step-200 bump has fallen monotonically: 1.0174 → 1.0580 → 0.9914 → 0.9513 → 0.9141 → 0.8483 → 0.7950 → **0.7486** at step 1,400, with about 100 steps plus the final save and chat check remaining.",
];

fn assistant_parts(harness: &str) -> Vec<WirePart> {
    let mut parts = vec![WirePart::text(
        "intro",
        "I’ll keep this attached to one CPU / Apple-Silicon baseline, inspect the exact local pipeline, make the portability and SFT safeguards part of the experiment before launch, then stay on the streamed output through tokenizer training, base training and evaluation, SFT, and the final chat check.",
    )];
    parts.push(task_list_part(
        harness,
        "todo",
        "Track the nanochat pipeline",
        &[
            ("Inspect and harden the CPU pipeline", "completed"),
            ("Run base training, evaluation, and SFT", "completed"),
            ("Confirm chat output and save results", "completed"),
        ],
    ));
    let (read_name, read_input, edit_name, edit_input, shell_name) = match harness {
        "claude-code" => (
            "Read",
            json!({ "file_path": "runs/runcpu.sh", "filePath": "runs/runcpu.sh" }),
            "Edit",
            json!({
                "file_path": "runs/runcpu.sh",
                "filePath": "runs/runcpu.sh",
                "old_string": "export NANOCHAT_BASE_DIR=\"$HOME/.cache/nanochat\"\nmkdir -p $NANOCHAT_BASE_DIR",
                "new_string": "export NANOCHAT_BASE_DIR=\"$PWD/.cache/nanochat\"\nexport UV_CACHE_DIR=\"$PWD/.cache/uv\"\nmkdir -p \"$NANOCHAT_BASE_DIR\" \"$UV_CACHE_DIR\""
            }),
            "Bash",
        ),
        "opencode" => (
            "read",
            json!({ "filePath": "runs/runcpu.sh" }),
            "bash",
            json!({ "command": "git diff --check && git diff --stat" }),
            "bash",
        ),
        _ => (
            "bash",
            json!({ "command": "sed -n '1,220p' runs/runcpu.sh scripts/chat_sft.py scripts/chat_cli.py" }),
            "edit",
            json!({ "changes": [
                { "path": "runs/runcpu.sh", "kind": "update" },
                { "path": "scripts/chat_sft.py", "kind": "update" },
                { "path": "scripts/chat_cli.py", "kind": "update" }
            ]}),
            "bash",
        ),
    };
    parts.push(tool_part(
        "inspect",
        read_name,
        read_input,
        Some("Inspected runs/runcpu.sh, scripts/chat_sft.py, and scripts/chat_cli.py. The pipeline trains a 32,768-token BPE tokenizer, a depth-6 base model for 5,000 steps, runs base evaluation, fine-tunes for 1,500 steps, then opens chat_cli against the saved SFT checkpoint."),
        Some("Inspect CPU pipeline"),
    ));
    parts.push(WirePart::text(
        "inspection-summary",
        "The project is fresh and the requested command supplies the complete execution recipe. Before spending compute, I’m making the final working safeguards explicit: keep nanochat and uv caches inside the checkout; exclude conversations longer than the 513-token SFT row; discard batches with no supervised targets; start SFT from the clean base weights with fresh optimizer state; disable periodic ChatCORE on unified memory while retaining validation BPB; and make the outer chat command re-enter the uv environment.",
    ));
    parts.push(tool_part(
        "portable-fixes",
        edit_name,
        edit_input,
        Some("Portable CPU/SFT safeguards applied."),
        Some("Apply portable safeguards"),
    ));
    parts.push(WirePart::text(
        "experiment-created",
        "The baseline is now fixed to one end-to-end command: the CPU/MPS pipeline followed immediately by the one-shot France prompt. I created the CPU / Apple-Silicon end-to-end baseline experiment and I’m launching its single local run now; I’ll keep reading the supervised log and surface every validation checkpoint as it lands.",
    ));
    parts.push(tool_part(
        "launch",
        shell_name,
        json!({ "command": "orx exp run demo_nanochat_cpu_v1 --backend local --force" }),
        Some("✓ Local run started.\nbackend  local\ncommand  bash runs/runcpu.sh && python -m scripts.chat_cli -p \"What is the capital of France?\""),
        Some("Launch the new baseline experiment"),
    ));
    parts.push(WirePart::text(
        "environment-setup",
        "The run cloned cleanly, created `.venv`, resolved 105 packages, and installed the CPU PyTorch stack. No training metric has appeared yet; setup is progressing normally.",
    ));
    parts.push(tool_part(
        "setup-log",
        shell_name,
        json!({ "command": "orx logs demo_nanochat_run_v1 --bytes 12000" }),
        Some("Using CPython 3.10.19\nCreating virtual environment at: .venv\nResolved 105 packages\nPrepared 40 packages\nInstalled 40 packages"),
        Some("Read environment setup"),
    ));
    parts.push(WirePart::text(
        "dataset-download",
        "Environment setup is complete. The dataset stage is active: all 9 ClimbMix shards are being downloaded in parallel. Training has not started yet, so there is no `val_bpb` to report.",
    ));
    parts.push(tool_part(
        "dataset-log",
        shell_name,
        json!({ "command": "orx logs demo_nanochat_run_v1 --bytes 16000" }),
        Some("Downloading 9 shards using 4 workers...\nSuccessfully downloaded shard_00000.parquet\nSuccessfully downloaded shard_00001.parquet\nSuccessfully downloaded shard_00002.parquet\nSuccessfully downloaded shard_00003.parquet\nSuccessfully downloaded shard_00004.parquet\nSuccessfully downloaded shard_00005.parquet\nSuccessfully downloaded shard_00006.parquet\nSuccessfully downloaded shard_00007.parquet\nSuccessfully downloaded shard_06542.parquet\nDone! Downloaded: 9/9 shards"),
        Some("Stream dataset setup"),
    ));
    parts.push(WirePart::text(
        "tokenizer-start",
        "All 9 dataset shards are downloaded. Tokenizer training has started with `max_chars=2,000,000,000`, `doc_cap=10,000`, and vocabulary size 32,768. The run is healthy and has moved past setup and data acquisition.",
    ));
    parts.push(tool_part(
        "tokenizer-log",
        shell_name,
        json!({ "command": "orx logs demo_nanochat_run_v1 --bytes 32000" }),
        Some("Starting BPE training: 32503 merges to compute\nProgress: 25% (8126/32503 merges)\nProgress: 50% (16251/32503 merges)\nProgress: 75% (24378/32503 merges)\nProgress: 100% (32503/32503 merges)\nFinished training: 32503 merges completed\nTraining time: 42.69s\nclimbmix-val 3024593 bytes 644939 tokens 4.69 bytes/token"),
        Some("Stream tokenizer training"),
    ));
    parts.push(WirePart::text(
        "tokenizer-complete",
        "Tokenizer training finished in 42.69s. Its validation text compression was 4.69 bytes/token (644,939 tokens over 3,024,593 bytes), about 1.2% better than GPT‑2 and 2.2% behind GPT‑4 on that sample. Base training is now running on MPS: 6 layers, 73.5M parameters, 5,000 iterations, 81.92M tokens. I’m watching every `val_bpb` checkpoint.",
    ));
    parts.push(tool_part(
        "base-log",
        shell_name,
        json!({ "command": "orx logs demo_nanochat_run_v1 --bytes 100000" }),
        Some("Autodetected device type: mps\nCOMPUTE_DTYPE: torch.float32 (auto-detected: no CUDA (CPU/MPS))\nWARNING: Flash Attention 3 not available, using PyTorch SDPA fallback\nVocab size: 32,768\nNumber of parameters: 73,454,976\nTraining for 5,000 steps / 81,920,000 tokens"),
        Some("Stream base training"),
    ));
    for (index, text) in BASE_PROGRESS.iter().enumerate() {
        parts.push(WirePart::text(format!("base-progress-{index}"), *text));
    }
    parts.push(tool_part(
        "base-eval-log",
        shell_name,
        json!({ "command": "orx logs demo_nanochat_run_v1 --bytes 100000" }),
        Some("Total training time: 131.55m\nMinimum validation bpb: 1.165758\n\nBPB Evaluation\ntrain bpb: 1.152185\nval bpb: 1.119301\n\nCORE Evaluation\nbigbench_qa_wikidata accuracy: 0.0000\nopenbook_qa accuracy: 0.2500\nwinogrande accuracy: 0.5625 | centered: 0.1250\nbigbench_operators accuracy: 0.0000"),
        Some("Read base evaluation"),
    ));
    parts.push(WirePart::text(
        "base-eval",
        "`base_eval` reported **train BPB 1.152185** and **validation BPB 1.119301**. The explicit capped CORE scores were OpenBookQA **0.2500**, Winogrande **0.5625** (centered 0.1250), Wikidata **0.0000**, and operators **0.0000**. The pipeline is now loading the clean step-5,000 base checkpoint for SFT.",
    ));
    parts.push(tool_part(
        "sft-log",
        shell_name,
        json!({ "command": "orx logs demo_nanochat_run_v1 --bytes 100000" }),
        Some("Autodetected device type: mps\nLoading model from $ORX_RUN_DIR/repo/.cache/nanochat/base_checkpoints/d6 with step 5000\nTraining mixture: 789,759 rows (MMLU x3, GSM8K x4)\nStep 00000 | Validation bpb: 1.0174\nstep 00001 | loss: 1.817033\nstep 00004 | loss: 6.121520\nstep 00017 | loss: 5.378817\nstep 00100 | loss: 3.2589"),
        Some("Stream supervised fine-tuning"),
    ));
    parts.push(WirePart::text(
        "sft-start",
        "SFT loaded the clean base weights with a fresh optimizer and reported initial validation BPB **1.0174**. The packing guard is keeping over-capacity conversations out of the 513-token rows, all supervised batches are finite, and periodic ChatCORE is disabled to keep Apple-Silicon unified-memory pressure bounded.",
    ));
    for (index, text) in SFT_PROGRESS.iter().enumerate() {
        parts.push(WirePart::text(format!("sft-progress-{index}"), *text));
    }
    parts.push(tool_part(
        "sft-complete-log",
        shell_name,
        json!({ "command": "orx logs demo_nanochat_run_v1 --bytes 100000" }),
        Some("step 01498 (99.93%) | loss: 2.052968 | total time: 39.04m\nstep 01499 (100.00%) | loss: 2.026530 | total time: 39.07m\nStep 01499 | Validation bpb: 0.7389\nSaved model parameters to: $ORX_RUN_DIR/repo/.cache/nanochat/chatsft_checkpoints/d6/model_001499.pt\nTotal training time: 39.07m\nMinimum validation bpb: 0.7389"),
        Some("Read final SFT checkpoint"),
    ));
    parts.push(WirePart::text(
        "sft-complete",
        "SFT finished successfully: final and minimum validation BPB is **0.7389**, and checkpoint `model_001499.pt` is saved. The outer command is re-entering nanochat’s uv environment and loading that checkpoint for the requested France prompt now.",
    ));
    parts.push(tool_part(
        "confirm",
        shell_name,
        json!({ "command": "orx logs demo_nanochat_run_v1 --bytes 12000" }),
        Some("Using completed SFT checkpoint model_001499.pt\nAutodetected device type: mps\nLoading model from $ORX_RUN_DIR/repo/.cache/nanochat/chatsft_checkpoints/d6 with step 1499\n\nAssistant:\n\nParis\nParis is a city known for its historical and cultural significance. The capital of France is Paris."),
        Some("Confirm the trained chat model"),
    ));
    parts.push(WirePart::text(
        "chat-confirmed",
        "The CLI loaded SFT checkpoint 1499 on MPS and answered correctly: **“Paris … The capital of France is Paris.”** Its later continuation became repetitive, reflecting the tiny local model, but inference worked and the requested answer was correct. I’m preserving the complete tokenizer, base, evaluation, SFT, and chat record in the artifact now.",
    ));
    let (report_tool, report_input) = match harness {
        "claude-code" => (
            "Bash",
            json!({ "command": "printf '%s\\n' '# nanochat CPU / Apple-Silicon pipeline results' 'Base validation BPB: 1.165758' 'SFT validation BPB: 0.7389' 'Chat confirmation: Paris' > cpu-apple-silicon-pipeline-results.md" }),
        ),
        "opencode" => (
            "bash",
            json!({ "command": "printf '%s\\n' '# nanochat CPU / Apple-Silicon pipeline results' 'Base validation BPB: 1.165758' 'SFT validation BPB: 0.7389' 'Chat confirmation: Paris' > cpu-apple-silicon-pipeline-results.md" }),
        ),
        _ => (
            edit_name,
            json!({ "changes": [{ "path": "cpu-apple-silicon-pipeline-results.md", "kind": "add" }] }),
        ),
    };
    parts.push(tool_part(
        "report",
        report_tool,
        report_input,
        Some("Results artifact saved."),
        Some("Save durable results"),
    ));
    parts.push(WirePart::text(
        "final",
        "Pipeline completed successfully.\n\n- Base training: 5,000 steps, 81.92M tokens, 131.55 min\n- Final base validation BPB: **1.165758**\n- Base evaluation train/validation BPB: **1.152185 / 1.119301**\n- CORE accuracy: OpenBookQA **0.2500**, Winogrande **0.5625**, Wikidata/operators **0.0000**\n- SFT: 1,500 steps, 39.07 min\n- Final/minimum SFT validation BPB: **0.7389**\n- Final run status: **done**\n\nThe CLI loaded SFT checkpoint 1499 on MPS and answered **Paris**. I preserved the full stitched run log and `cpu-apple-silicon-pipeline-results.md` in the project artifact.",
    ));
    parts
}

fn figure_assistant_parts(harness: &str) -> Vec<WirePart> {
    let (read_tool, shell_tool, edit_tool, read_input, edit_input) = match harness {
        "claude-code" => (
            "Read",
            "Bash",
            "Edit",
            json!({
                "file_path": "cpu-apple-silicon-pipeline-results.md",
                "filePath": "cpu-apple-silicon-pipeline-results.md"
            }),
            json!({
                "file_path": "$TMPDIR/create_nanochat_publication_figures.py",
                "filePath": "$TMPDIR/create_nanochat_publication_figures.py",
                "old_string": "",
                "new_string": "Parse recorded base and SFT metrics and render four SVG figures."
            }),
        ),
        "opencode" => (
            "read",
            "bash",
            "bash",
            json!({ "filePath": "cpu-apple-silicon-pipeline-results.md" }),
            json!({ "command": "printf '%s\n' '# deterministic SVG renderer' > $TMPDIR/create_nanochat_publication_figures.py" }),
        ),
        _ => (
            "bash",
            "bash",
            "edit",
            json!({ "command": "sed -n '1,220p' cpu-apple-silicon-pipeline-results.md" }),
            json!({ "changes": [{
                "path": "$TMPDIR/create_nanochat_publication_figures.py",
                "kind": "add"
            }] }),
        ),
    };
    let mut parts = vec![WirePart::text(
        "figure-intro",
        "I’ll treat the recorded logs as the source of truth, keep base and SFT validation separate, and produce only the four quantitative SVGs requested. I’m first checking the report, complete per-step logs, and exact benchmark labels, then I’ll build the plots with one consistent Helvetica Neue academic style and visually inspect the rendered output.",
    )];
    parts.push(task_list_part(
        harness,
        "figure-todos",
        "Track figure generation",
        &[
            ("Parse base and SFT logs", "completed"),
            ("Generate four SVG-only figures", "completed"),
            ("Inspect and validate final artifacts", "completed"),
        ],
    ));
    parts.push(tool_part(
        "figure-read-report",
        read_tool,
        read_input,
        Some("Recorded phases: 5,000 base steps, 1,499 SFT loss rows, base and SFT validation checkpoints, Apple-Silicon throughput, and four CORE benchmarks."),
        Some("Inspect the recorded experiment"),
    ));
    parts.push(tool_part(
        "figure-write-script",
        edit_tool,
        edit_input,
        Some("Created a deterministic Matplotlib renderer with Helvetica Neue, SVG text, colorblind-safe blue/orange/green, thin neutral grids, and no raster output."),
        Some("Build the publication figure renderer"),
    ));
    parts.push(tool_part(
        "figure-parse-logs",
        shell_tool,
        json!({ "command": "orx logs demo_nanochat_run_v1 --bytes 2000000 | python $TMPDIR/create_nanochat_publication_figures.py --summarize" }),
        Some("base loss rows: 5000\nsft loss rows: 1499\nbase validation: 3.195800 → 1.165758\nsft validation: 1.0174 → 0.7389\nbase median throughput: 10466 tok/s\nsft median throughput: 10551 tok/s\nCORE rows: 4"),
        Some("Parse complete training logs"),
    ));
    parts.push(WirePart::text(
        "figure-data-summary",
        "The logs have the full resolution needed for publication plots: 5,000 base loss points and 1,499 SFT loss points, plus every validation checkpoint. Median throughput is 10,466 tok/s for base training and 10,551 tok/s for SFT. CORE labels and values are preserved exactly: Wikidata QA 0.0000, OpenBookQA 0.2500, Winogrande 0.5625 (centered 0.1250), and Operators 0.0000.",
    ));
    parts.push(tool_part(
        "figure-render",
        shell_tool,
        json!({ "command": "MPLBACKEND=Agg python $TMPDIR/create_nanochat_publication_figures.py" }),
        Some("wrote nanochat-base-training-curves.svg\nwrote nanochat-sft-training-curves.svg\nwrote nanochat-training-throughput.svg\nwrote nanochat-core-evaluation.svg"),
        Some("Render four SVG figures"),
    ));
    parts.push(WirePart::text(
        "figure-rendered",
        "All four figures are rendered. Base and SFT each pair the complete raw loss trace with the requested rolling mean and a separate validation-BPB panel; throughput uses normalized training progress so the unequal phase lengths remain comparable; CORE uses exact labeled values rather than decorative callouts.",
    ));
    parts.push(tool_part(
        "figure-inspect",
        shell_tool,
        json!({ "command": "python $TMPDIR/create_nanochat_publication_figures.py --inspect nanochat-*.svg" }),
        Some("Visually inspected all four figures at full size. Axes, legends, annotations, and line weights are clear; no clipping or overlap detected."),
        Some("Inspect rendered figures"),
    ));
    parts.push(tool_part(
        "figure-validate",
        shell_tool,
        json!({ "command": "python $TMPDIR/create_nanochat_publication_figures.py --validate nanochat-*.svg --font 'Helvetica Neue'" }),
        Some("4 valid SVG documents\nall figures reference Helvetica Neue\nall metric labels verified\n0 PNG artifacts"),
        Some("Validate final artifacts"),
    ));
    let artifact_link = |name: &str| format!("[{name}](artifacts/{name})");
    parts.push(WirePart::text(
        "figure-final",
        format!(
            "Completed the requested SVG-only figure set.\n\n- **Base training:** loss falls from about 10.40 to 3.75 over 5,000 steps; validation BPB improves from 3.1958 to 1.1658, with post-training evaluation at 1.1193.\n- **SFT:** loss finishes near 2.03; validation BPB briefly rises from 1.0174 to 1.0580 at step 200, then falls steadily to 0.7389.\n- **Throughput:** median 10,466 tok/s for base and 10,551 tok/s for SFT on Apple Silicon.\n- **CORE:** Winogrande leads at 0.5625 accuracy (0.1250 centered); OpenBookQA is 0.2500; Wikidata QA and Operators are 0.0000.\n\nFiles:\n\n- {}\n- {}\n- {}\n- {}",
            artifact_link("nanochat-base-training-curves.svg"),
            artifact_link("nanochat-sft-training-curves.svg"),
            artifact_link("nanochat-training-throughput.svg"),
            artifact_link("nanochat-core-evaluation.svg"),
        ),
    ));
    parts
}

fn literature_assistant_parts(harness: &str) -> Vec<WirePart> {
    let (read_tool, shell_tool, write_tool) = match harness {
        "claude-code" => ("Read", "Bash", "Edit"),
        "opencode" => ("read", "bash", "bash"),
        _ => ("bash", "bash", "edit"),
    };
    let read_input = |path: &str| match harness {
        "claude-code" => json!({ "file_path": path, "filePath": path }),
        "opencode" => json!({ "filePath": path }),
        _ => json!({ "command": format!("sed -n '1,280p' {path}") }),
    };
    let mut parts = vec![WirePart::text(
        "literature-intro",
        "I’m using the literature-search workflow first, then I’ll inspect the nanochat experiment history so the recommendation is grounded in both scaling literature and the actual recorded runs.",
    )];
    parts.push(task_list_part(
        harness,
        "literature-todos",
        "Track the bottleneck diagnosis",
        &[
            ("Inspect the recorded nanochat evidence", "completed"),
            ("Review scaling and alignment literature", "completed"),
            ("Recommend one controlled next experiment", "completed"),
        ],
    ));
    for (id, path, title) in [
        (
            "literature-skill",
            ".agents/skills/orx-lit-review/SKILL.md",
            "Load literature search workflow",
        ),
        (
            "evidence-skill",
            ".agents/skills/orx-evidence/SKILL.md",
            "Load evidence workflow",
        ),
        (
            "experiment-tree-skill",
            ".agents/skills/orx-experiment-tree/SKILL.md",
            "Load experiment comparison workflow",
        ),
    ] {
        parts.push(tool_part(
            id,
            read_tool,
            read_input(path),
            Some("Workflow loaded."),
            Some(title),
        ));
    }
    parts.push(WirePart::text(
        "literature-comparability",
        "The next-experiment recommendation depends on keeping the comparison identifiable, so I’ll separate the project’s measured evidence from the broader literature and change only one factor in the proposed follow-up.",
    ));
    for (id, command, output, title) in [
        (
            "inspect-project",
            "git branch -a && git status --short --branch && orx projects",
            "nanochat\nbaseline branch: main\nexperiment: CPU Apple-Silicon end-to-end baseline",
            "Inspect nanochat project history",
        ),
        (
            "inspect-run",
            "orx runs demo_nanochat_v1 && orx project view demo_nanochat_v1",
            "1 successful run\ncommand: bash runs/runcpu.sh\nmodel: depth 6, 73.5M parameters\ntraining: 81.92M tokens",
            "Inspect completed runs",
        ),
        (
            "inspect-base-log",
            "orx logs demo_nanochat_run_v1 --bytes 1000000 | rg 'Validation bpb|CORE|step 05000'",
            "Step 04000 | Validation bpb: 1.187774\nStep 04500 | Validation bpb: 1.1743\nStep 05000 | Validation bpb: 1.165758\nCORE: Wikidata 0.0000, OpenBookQA 0.2500, Winogrande 0.5625 (centered 0.1250), Operators 0.0000",
            "Read base-training evidence",
        ),
        (
            "inspect-sft-log",
            "orx logs demo_nanochat_run_v1 --bytes 1000000 | rg 'SFT|Validation bpb|Paris'",
            "SFT validation bpb: 1.0174 → 0.7389\nfinal checkpoint: model_001499.pt\nfixed prompt answer: Paris, followed by repetitive continuation",
            "Read SFT and generation evidence",
        ),
    ] {
        parts.push(tool_part(
            id,
            shell_tool,
            json!({ "command": command }),
            Some(output),
            Some(title),
        ));
    }
    parts.push(WirePart::text(
        "literature-project-evidence",
        "The project evidence already points toward undertraining: base validation BPB was still improving at the final checkpoint, while SFT fit its held-out token distribution much better without producing robust free-running behavior. I’m checking scaling, small-model data quality, and alignment literature before ranking the causes.",
    ));
    for (index, query) in [
        "TinyStories small language models coherent text 10 million parameters",
        "Training Compute-Optimal Large Language Models Hoffmann tokens per parameter",
        "scaling laws downstream task performance small language models pretraining tokens",
        "language model supervised fine tuning repetition overfitting low validation loss generation degeneration",
        "instruction tuning small pretrained language models capabilities cannot create knowledge",
        "Textbooks Are All You Need phi-1 data quality small language model",
        "Scaling laws neural language models downstream performance model size data Kaplan",
        "LIMA Less Is More for Alignment fine tuning knowledge learned during pretraining",
    ]
    .iter()
    .enumerate()
    {
        parts.push(tool_part(
            &format!("literature-search-{index}"),
            shell_tool,
            json!({ "command": format!("orx discover keyword \"{query}\" --limit 8") }),
            Some("Relevant papers and passages retrieved."),
            Some("Search the literature"),
        ));
    }
    for (id, paper, output) in [
        (
            "paper-tinystories",
            "2305.07759",
            "TinyStories: small models can generate coherently on a deliberately constrained, high-quality distribution.",
        ),
        (
            "paper-kaplan",
            "2001.08361",
            "Scaling Laws for Neural Language Models: performance follows predictable model/data/compute trends.",
        ),
        (
            "paper-phi",
            "2306.11644",
            "Textbooks Are All You Need: curated educational data substantially improves small-model sample efficiency.",
        ),
        (
            "paper-lima",
            "2305.11206",
            "LIMA: most knowledge is learned during pretraining; SFT largely teaches response format and alignment.",
        ),
        (
            "paper-chinchilla",
            "2203.15556",
            "Chinchilla: compute-optimal training scales model parameters and training tokens together; the canonical point is about 20 tokens per parameter.",
        ),
    ] {
        parts.push(tool_part(
            id,
            shell_tool,
            json!({ "command": format!("orx paper {paper}") }),
            Some(output),
            Some("Read primary paper"),
        ));
    }
    parts.push(tool_part(
        "inspect-scaling-code",
        read_tool,
        read_input("scripts/base_train.py"),
        Some("nanochat default target_param_data_ratio = 12\nscaling parameters = transformer matrices + lm_head = 23,199,960"),
        Some("Inspect nanochat scaling convention"),
    ));
    parts.push(tool_part(
        "calculate-ablation",
        shell_tool,
        json!({ "command": "python -c 'print(23199960 * 12, (23199960 * 12) // 16384)'" }),
        Some("278396928 tokens\n16992 steps\ncurrent ratio: 81920000 / 23199960 = 3.53"),
        Some("Calculate the controlled token ablation"),
    ));
    parts.push(WirePart::text(
        "literature-convergence",
        "The evidence is converging: the base model’s validation curve was still improving at the final token, and its effective token/parameter ratio was 3.53 versus nanochat’s own default target of 12. I’m recording the literature-backed diagnosis as a durable project note before giving the concise recommendation.",
    ));
    let report_path = "artifacts/nanochat-bottleneck-diagnosis.md";
    let write_input = match harness {
        "claude-code" => json!({
            "file_path": report_path,
            "filePath": report_path,
            "old_string": "",
            "new_string": BOTTLENECK_REPORT,
        }),
        "opencode" => json!({
            "command": "mkdir -p artifacts && cp $TMPDIR/nanochat-bottleneck-diagnosis.md artifacts/nanochat-bottleneck-diagnosis.md",
        }),
        _ => json!({
            "changes": [{ "path": report_path, "kind": "add" }],
        }),
    };
    parts.push(tool_part(
        "write-literature-report",
        write_tool,
        write_input,
        Some("Saved nanochat-bottleneck-diagnosis.md."),
        Some("Save the literature diagnosis"),
    ));
    parts.push(tool_part(
        "verify-literature-report",
        shell_tool,
        json!({ "command": "wc -l artifacts/nanochat-bottleneck-diagnosis.md" }),
        Some("38 artifacts/nanochat-bottleneck-diagnosis.md"),
        Some("Verify the durable report"),
    ));
    parts.push(WirePart::text(
        "literature-final",
        format!(
            "The main bottleneck is insufficient pretraining. Model size sets the eventual ceiling; the SFT recipe likely worsens repetition but cannot manufacture knowledge the base model never learned.\n\nWhy:\n\n- The model saw only 81.9M tokens. Under nanochat’s own scaling convention, that is 3.53 tokens per scaling parameter, versus its default target of 12.\n- Base validation BPB was still improving at the endpoint: 1.1878 at step 4,000 → 1.1743 at 4,500 → 1.1658 at 5,000. The model had not converged.\n- [Chinchilla](https://arxiv.org/abs/2203.15556) establishes that substantially scaling training tokens is generally preferable to leaving a model undertrained. Its canonical ~20-token ratio is not directly comparable because nanochat counts parameters differently, but the direction is unambiguous.\n- [LIMA](https://arxiv.org/abs/2305.11206) supports the view that SFT mostly exposes capabilities acquired during pretraining. It also reports that validation perplexity can continue improving after generation quality peaks—matching your falling SFT loss but poor free-running behavior.\n- [TinyStories](https://arxiv.org/abs/2305.07759) shows that 73.5M parameters are sufficient for coherent generation on a constrained, high-quality distribution. Thus size alone does not explain the repetition. Conversely, broad MMLU/GSM8K performance at this size will remain limited.\n- [Textbooks Are All You Need](https://arxiv.org/abs/2306.11644) makes pretraining-data quality an important subsequent axis, but your run is too undertrained to diagnose quality cleanly yet.\n\nRun this next:\n\n- Train the identical d6 model from scratch with the same data and recipe, changing only the pretraining horizon to `--target-param-data-ratio=12`.\n- That gives 278,396,928 tokens or 16,992 steps—3.4× the current exposure.\n- Evaluate checkpoints at 5,000, ~11,000, and 16,992 steps using fixed prompts, base BPB, CORE, and a repetition metric.\n- Apply the identical 1,500-step SFT recipe only to the final checkpoint and compare ChatCORE, MMLU, GSM8K, and generation.\n\nDo not change SFT simultaneously; that would obscure whether additional pretraining solved the problem. If repetition remains after the longer-pretrained base improves, the following round should test SFT early stopping/lower learning rate using generation and ChatCORE—not validation loss—as the selection criterion.\n\nI saved the full diagnosis as the project artifact [{}]({}).",
            "nanochat-bottleneck-diagnosis.md",
            report_path,
        ),
    ));
    parts
}

/// The harness's own task-list tool call, in that harness's wire shape, so the
/// demo transcript shows the same step checklist a live session would.
fn task_list_part(harness: &str, id: &str, title: &str, steps: &[(&str, &str)]) -> WirePart {
    let (tool, input) = match harness {
        "claude-code" => (
            "TodoWrite",
            json!({ "todos": steps.iter().map(|(content, status)| {
                json!({ "content": content, "status": status, "activeForm": content })
            }).collect::<Vec<_>>() }),
        ),
        "opencode" => (
            "todowrite",
            json!({ "todos": steps.iter().map(|(content, status)| {
                json!({ "content": content, "status": status, "priority": "high" })
            }).collect::<Vec<_>>() }),
        ),
        _ => (
            "update_plan",
            json!({ "plan": steps.iter().map(|(step, status)| {
                json!({ "step": step, "status": status })
            }).collect::<Vec<_>>() }),
        ),
    };
    tool_part(id, tool, input, None, Some(title))
}

fn tool_part(
    id: &str,
    tool: &str,
    input: Value,
    output: Option<&str>,
    title: Option<&str>,
) -> WirePart {
    WirePart {
        id: id.into(),
        kind: "tool".into(),
        text: None,
        tool: Some(tool.into()),
        state: Some(WireToolState {
            status: "completed".into(),
            input: Some(input),
            output: output.map(str::to_string),
            error: None,
            title: title.map(str::to_string),
        }),
        prompt: None,
        children: Vec::new(),
    }
}

fn install_repository(repo: &Path, bare: &Path) -> Result<String> {
    if repo.exists() {
        validate_worktree(repo)?;
    } else {
        let parent = repo
            .parent()
            .ok_or_else(|| anyhow!("demo repository has no parent directory"))?;
        std::fs::create_dir_all(parent)?;
        let tmp = parent.join(format!(".nanochat-demo-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp)?;
        let result = build_worktree(&tmp)
            .and_then(|_| validate_worktree(&tmp))
            .and_then(|_| match std::fs::rename(&tmp, repo) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    validate_worktree(repo)
                }
                Err(error) => Err(anyhow!("could not install demo repository: {error}")),
            });
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&tmp);
        }
        result?;
    }
    ensure_follow_up_branches(repo)?;
    ensure_local_origin(repo, bare)?;
    git(repo, &["rev-parse", BRANCH])
}

// Idle follow-up experiments start at the completed baseline commit. Also covers a
// cached demo clone that predates them when the project is seeded again after a DB reset.
fn ensure_follow_up_branches(repo: &Path) -> Result<()> {
    for branch in [LR_PROBE_BRANCH, VOCAB_PROBE_BRANCH] {
        if git(
            repo,
            &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
        )
        .is_err()
        {
            git(repo, &["branch", branch, BRANCH])?;
        }
    }
    Ok(())
}

fn build_worktree(root: &Path) -> Result<()> {
    write_assets::<BaseAssets>(root)?;
    set_executable(root.join("runs/runcpu.sh"))?;
    git(root, &["init", "--object-format=sha1", "-b", "main"])?;
    git(root, &["config", "core.autocrlf", "false"])?;
    git(root, &["config", "core.filemode", "true"])?;
    git(root, &["add", "-A"])?;
    commit(root, "Import nanochat demo baseline")?;
    git(root, &["checkout", "-b", BRANCH])?;
    write_assets::<ExperimentAssets>(root)?;
    set_executable(root.join("runs/runcpu.sh"))?;
    git(root, &["add", "-A"])?;
    commit(root, "Make the CPU pipeline portable and memory-safe")?;
    git(root, &["checkout", "main"])?;
    Ok(())
}

fn ensure_local_origin(repo: &Path, bare: &Path) -> Result<()> {
    if bare.exists() {
        validate_bare_origin(bare)?;
    } else {
        let parent = bare
            .parent()
            .ok_or_else(|| anyhow!("demo origin has no parent directory"))?;
        std::fs::create_dir_all(parent)?;
        let tmp = parent.join(format!(".nanochat-demo-origin-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp)?;
        git(&tmp, &["init", "--bare", "--object-format=sha1"])?;
        git(
            repo,
            &[
                "push",
                "--no-verify",
                tmp.to_string_lossy().as_ref(),
                "main",
                BRANCH,
                LR_PROBE_BRANCH,
                VOCAB_PROBE_BRANCH,
            ],
        )?;
        git(&tmp, &["symbolic-ref", "HEAD", "refs/heads/main"])?;
        if let Err(error) = std::fs::rename(&tmp, bare) {
            if error.kind() != std::io::ErrorKind::AlreadyExists {
                let _ = std::fs::remove_dir_all(&tmp);
                return Err(anyhow!("could not install demo Git origin: {error}"));
            }
            validate_bare_origin(bare)?;
        }
    }
    let origin = bare.to_string_lossy();
    if git(repo, &["remote", "get-url", "origin"]).is_ok() {
        git(repo, &["remote", "set-url", "origin", origin.as_ref()])?;
    } else {
        git(repo, &["remote", "add", "origin", origin.as_ref()])?;
    }
    git(
        repo,
        &[
            "push",
            "--no-verify",
            "-u",
            "origin",
            "main",
            BRANCH,
            LR_PROBE_BRANCH,
            VOCAB_PROBE_BRANCH,
        ],
    )?;
    validate_bare_origin(bare)?;
    Ok(())
}

fn validate_bare_origin(bare: &Path) -> Result<()> {
    let baseline = git(bare, &["rev-parse", "refs/heads/main"]);
    let experiment = git(bare, &["rev-parse", &format!("refs/heads/{BRANCH}")]);
    let is_bare = git(bare, &["rev-parse", "--is-bare-repository"]);
    let head = git(bare, &["symbolic-ref", "HEAD"]);
    if !bare.join("HEAD").is_file()
        || !matches!(baseline.as_deref(), Ok(value) if value == BASELINE_SHA)
        || !matches!(experiment.as_deref(), Ok(value) if value == EXPERIMENT_SHA)
        || !matches!(is_bare.as_deref(), Ok("true"))
        || !matches!(head.as_deref(), Ok("refs/heads/main"))
    {
        return Err(anyhow!(
            "the reserved demo origin at {} does not contain the expected OpenResearch refs; move it aside and retry onboarding",
            bare.display()
        ));
    }
    Ok(())
}

fn validate_worktree(repo: &Path) -> Result<()> {
    let baseline = git(repo, &["rev-parse", "refs/heads/main"]);
    let experiment = git(repo, &["rev-parse", &format!("refs/heads/{BRANCH}")]);
    let clean = git(repo, &["status", "--porcelain"]);
    let ancestry = git(
        repo,
        &["merge-base", "--is-ancestor", BASELINE_SHA, EXPERIMENT_SHA],
    );
    if !repo.join(".git").is_dir()
        || !matches!(baseline.as_deref(), Ok(value) if value == BASELINE_SHA)
        || !matches!(experiment.as_deref(), Ok(value) if value == EXPERIMENT_SHA)
        || !matches!(clean.as_deref(), Ok(""))
        || ancestry.is_err()
    {
        return Err(anyhow!(
            "the reserved demo path at {} already exists but is not the OpenResearch nanochat demo; move it aside and retry onboarding",
            repo.display()
        ));
    }
    Ok(())
}

fn write_assets<T: RustEmbed>(root: &Path) -> Result<()> {
    for name in T::iter() {
        let asset = T::get(name.as_ref())
            .ok_or_else(|| anyhow!("embedded demo asset disappeared: {name}"))?;
        let path = root.join(name.as_ref());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, asset.data.as_ref())?;
    }
    Ok(())
}

fn commit(repo: &Path, message: &str) -> Result<()> {
    git(
        repo,
        &[
            "-c",
            "user.name=OpenResearch Demo",
            "-c",
            "user.email=demo@openresearch.sh",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "-m",
            message,
        ],
    )?;
    Ok(())
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let mut command = Command::new("git");
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_COMMON_DIR",
        "GIT_CEILING_DIRECTORIES",
        "GIT_DISCOVERY_ACROSS_FILESYSTEM",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
        "GIT_TEMPLATE_DIR",
    ] {
        command.env_remove(name);
    }
    let out = command
        .current_dir(dir)
        .args([
            "-c",
            "core.attributesFile=/dev/null",
            "-c",
            "core.excludesFile=/dev/null",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "OpenResearch Demo")
        .env("GIT_AUTHOR_EMAIL", "demo@openresearch.sh")
        .env("GIT_COMMITTER_NAME", "OpenResearch Demo")
        .env("GIT_COMMITTER_EMAIL", "demo@openresearch.sh")
        .env("GIT_AUTHOR_DATE", "2026-08-04T12:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-08-04T12:00:00Z")
        .output()
        .map_err(|e| anyhow!("could not run git: {e}"))?;
    if !out.status.success() {
        return Err(anyhow!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn set_executable(path: PathBuf) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&path)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_variants_are_one_turn_and_use_native_tool_names() {
        let cases: [(&str, &[&str]); 3] = [
            ("claude-code", &["Read", "Edit", "Bash", "TodoWrite"]),
            ("codex", &["bash", "edit", "update_plan"]),
            ("opencode", &["read", "bash", "todowrite"]),
        ];
        for (harness, expected) in cases {
            let parts = assistant_parts(harness);
            let encoded = serde_json::to_string(&parts).unwrap();
            let decoded: Vec<WirePart> = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded.len(), parts.len());
            assert!(
                parts.len() < 40,
                "{harness} transcript regressed to per-checkpoint spam"
            );
            assert_eq!(
                parts
                    .iter()
                    .filter(|part| part.id.starts_with("base-progress-"))
                    .count(),
                BASE_PROGRESS.len()
            );
            assert_eq!(
                parts
                    .iter()
                    .filter(|part| part.id.starts_with("sft-progress-"))
                    .count(),
                SFT_PROGRESS.len()
            );
            let names: Vec<&str> = parts
                .iter()
                .filter_map(|part| part.tool.as_deref())
                .collect();
            for tool in expected {
                assert!(names.contains(tool), "{harness} missing {tool}: {names:?}");
            }
            let allowed: &[&str] = match harness {
                "claude-code" => &["Read", "Edit", "Bash", "TodoWrite"],
                "opencode" => &["read", "bash", "todowrite"],
                _ => &["bash", "edit", "update_plan"],
            };
            assert!(names.iter().all(|name| allowed.contains(name)));
            assert_eq!(parts.iter().filter(|part| part.kind == "prompt").count(), 0);
            for command in parts
                .iter()
                .filter_map(|part| part.state.as_ref())
                .filter_map(|state| state.input.as_ref())
                .filter_map(|input| input.get("command"))
                .filter_map(Value::as_str)
            {
                assert!(!command.contains("--slug"));
                assert!(!command.contains("--tail"));
                assert_ne!(command, "apply the reviewed portability and SFT safeguards");
                assert_ne!(command, "write the consolidated result artifact");
            }
            if harness == "claude-code" {
                for part in parts
                    .iter()
                    .filter(|part| matches!(part.tool.as_deref(), Some("Read") | Some("Edit")))
                {
                    assert!(part
                        .state
                        .as_ref()
                        .and_then(|state| state.input.as_ref())
                        .and_then(|input| input.get("filePath"))
                        .is_some());
                }
            }
        }
    }

    #[test]
    fn supplementary_transcripts_use_portable_native_parts() {
        for harness in ["claude-code", "codex", "opencode"] {
            for parts in [
                figure_assistant_parts(harness),
                literature_assistant_parts(harness),
            ] {
                let encoded = serde_json::to_string(&parts).unwrap();
                assert!(!encoded.contains("](/"));
                assert!(!encoded.contains("parse-nanochat-metrics"));
                assert!(!encoded.contains("inspect-svg"));
                assert!(!encoded.contains("validate-svg-artifacts"));
                if harness == "claude-code" {
                    for part in parts
                        .iter()
                        .filter(|part| matches!(part.tool.as_deref(), Some("Read") | Some("Edit")))
                    {
                        let input = part.state.as_ref().unwrap().input.as_ref().unwrap();
                        assert!(input.get("file_path").is_some());
                        assert!(input.get("filePath").is_some());
                    }
                }
            }
        }
    }

    #[test]
    fn every_demo_session_recovers_from_the_experiment_commit() {
        for session_id in [SESSION_ID, FIGURE_SESSION_ID, LITERATURE_SESSION_ID] {
            assert_eq!(
                session_start_ref(OWNER, REPO, session_id),
                Some(EXPERIMENT_SHA)
            );
        }
    }

    #[test]
    fn repository_and_snapshot_seed_are_idempotent() {
        let root = std::env::temp_dir().join(format!("orx-demo-test-{}", uuid::Uuid::new_v4()));
        let data = root.join("data");
        let repo = root.join("cache/repos").join(OWNER).join(REPO);
        let store = Store::open_at(data.clone()).unwrap();
        let selection = DemoSelection {
            harness: "codex".into(),
            model: None,
            permission_mode: None,
            reasoning_level: None,
        };
        let first = seed_at(&store, &data, &repo, selection.clone()).unwrap();
        let user_notes = data.join("files/nanochat/user-notes.md");
        std::fs::write(&user_notes, "# User notes\n").unwrap();
        let second = seed_at(
            &store,
            &data,
            &repo,
            DemoSelection {
                harness: "claude-code".into(),
                ..selection
            },
        )
        .unwrap();
        assert_eq!(first.project.id, second.project.id);
        assert_eq!(second.selection.harness, "codex");
        assert_eq!(store.list_local_projects().unwrap().len(), 1);
        assert_eq!(
            store.get_local_project(PROJECT_ID).unwrap().unwrap().name,
            "nanochat (demo)"
        );
        let experiments = store.list_experiments_by_project(PROJECT_ID).unwrap();
        assert_eq!(experiments.len(), 3);
        for follow_up in &experiments[1..] {
            assert_eq!(
                follow_up.parent_experiment_id.as_deref(),
                Some(EXPERIMENT_ID)
            );
            assert_eq!(follow_up.agent_status, "idle");
            assert_eq!(
                git(&repo, &["rev-parse", &follow_up.branch_name]).unwrap(),
                EXPERIMENT_SHA
            );
            assert_eq!(
                git(
                    &data.join("demo-repos/nanochat.git"),
                    &[
                        "rev-parse",
                        &format!("refs/heads/{}", follow_up.branch_name)
                    ]
                )
                .unwrap(),
                EXPERIMENT_SHA
            );
        }
        assert_eq!(store.list_runs_by_project(PROJECT_ID).unwrap().len(), 1);
        assert_eq!(
            store
                .list_chat_sessions_by_project(PROJECT_ID)
                .unwrap()
                .len(),
            3
        );
        let run = store.get_run(RUN_ID).unwrap().unwrap();
        assert_eq!(run.status, "done");
        assert_eq!(run.exit_code, Some(0));
        assert_eq!(run.chat_session_id, None);
        assert_eq!(
            run.commit_sha.as_deref(),
            Some(git(&repo, &["rev-parse", BRANCH]).unwrap().as_str())
        );
        let messages = store.list_chat_messages(SESSION_ID).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[1].role, "assistant");
        // Seeding runs long after the tree backfill, so an unparented reply here
        // would stay unparented: the first message the user sends would start a
        // second branch root and hide the whole seeded transcript.
        assert_eq!(messages[0].parent_id, None);
        assert_eq!(
            messages[1].parent_id.as_deref(),
            Some(messages[0].id.as_str())
        );
        let session = store.get_chat_session(SESSION_ID).unwrap().unwrap();
        assert_eq!(
            session.active_leaf_id.as_deref(),
            Some(messages[1].id.as_str())
        );
        let assistant: Vec<WirePart> = serde_json::from_str(&messages[1].parts_json).unwrap();
        assert!(!assistant.is_empty());
        assert!(!messages[1].parts_json.contains("/Users/"));
        let figure_messages = store.list_chat_messages(FIGURE_SESSION_ID).unwrap();
        assert_eq!(figure_messages.len(), 2);
        let figure_assistant: Vec<WirePart> =
            serde_json::from_str(&figure_messages[1].parts_json).unwrap();
        assert_eq!(
            figure_assistant
                .iter()
                .filter_map(|part| part.text.as_deref())
                .filter(|text| text.contains("artifacts/nanochat-"))
                .count(),
            1
        );
        assert!(!figure_messages[1]
            .parts_json
            .contains(data.to_string_lossy().as_ref()));
        assert_eq!(
            figure_messages[0]
                .parts_json
                .matches(FIGURE_USER_PROMPT)
                .count(),
            1
        );
        let literature_messages = store.list_chat_messages(LITERATURE_SESSION_ID).unwrap();
        assert_eq!(literature_messages.len(), 2);
        assert!(literature_messages[0]
            .parts_json
            .contains("My 73.5M-parameter nanochat model"));
        assert!(literature_messages[1]
            .parts_json
            .contains("The main bottleneck is insufficient pretraining"));
        assert!(!literature_messages[1].parts_json.contains("/Users/"));
        assert!(!literature_messages[1]
            .parts_json
            .contains(data.to_string_lossy().as_ref()));
        assert!(repo.join(".git").is_dir());
        let bare = data.join("demo-repos/nanochat.git");
        assert!(bare.join("HEAD").is_file());
        assert_eq!(
            git(&bare, &["symbolic-ref", "HEAD"]).unwrap(),
            "refs/heads/main"
        );
        assert_eq!(
            git(&repo, &["remote", "get-url", "origin"]).unwrap(),
            bare.to_string_lossy()
        );
        let changed = git(&repo, &["diff", "--name-only", "main", BRANCH]).unwrap();
        assert_eq!(
            changed.lines().collect::<Vec<_>>(),
            [
                ".gitignore",
                "runs/runcpu.sh",
                "scripts/chat_cli.py",
                "scripts/chat_sft.py"
            ]
        );
        assert!(data
            .join("files/nanochat/cpu-apple-silicon-pipeline-results.md")
            .is_file());
        assert_eq!(
            std::fs::read_to_string(&user_notes).unwrap(),
            "# User notes\n"
        );
        assert_eq!(
            std::fs::read_dir(data.join("files/nanochat"))
                .unwrap()
                .count(),
            8
        );
        for name in FigureAssets::iter() {
            assert!(data.join("files/nanochat").join(name.as_ref()).is_file());
        }
        for name in EvidenceAssets::iter() {
            assert!(data
                .join("files/nanochat/evidence")
                .join(name.as_ref())
                .is_file());
        }
        assert_eq!(
            std::fs::metadata(data.join("files/nanochat/evidence/tokenizer/tokenizer.pkl"))
                .unwrap()
                .len(),
            412_105
        );
        assert!(data
            .join("files/nanochat/nanochat-bottleneck-diagnosis.md")
            .is_file());
        let log = std::fs::read_to_string(data.join(format!("run-logs/{RUN_ID}.log"))).unwrap();
        assert!(log.lines().count() > 7_000);
        assert!(log.contains("Finished training: 32503 merges completed"));
        assert!(log.contains("Step 05000 | Validation bpb: 1.165758"));
        assert!(log.contains("Step 01499 | Validation bpb: 0.7389"));
        assert!(log.contains("The capital of France is Paris"));
        assert!(!log.contains("/Users/"));
        assert!(!log.contains("Traceback"));
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn demo_seed_preserves_legacy_user_artifacts_with_the_nanochat_slug() {
        let root = std::env::temp_dir().join(format!("orx-demo-test-{}", uuid::Uuid::new_v4()));
        let data = root.join("data");
        let repo = root.join("cache/repos").join(OWNER).join(REPO);
        let store = Store::open_at(data.clone()).unwrap();
        store
            .create_local_project(&LocalProject {
                id: "user-nanochat".into(),
                name: "nanochat".into(),
                slug: PROJECT_SLUG.into(),
                github_owner: String::new(),
                github_repo: String::new(),
                github_sync_enabled: false,
                baseline_branch: "main".into(),
                repo_path: root.join("user-nanochat").to_string_lossy().into_owned(),
                run_command: None,
                paper_id: None,
                created_at: 1,
                updated_at: 1,
            })
            .unwrap();
        let user_files = data.join("files").join(PROJECT_SLUG);
        std::fs::create_dir_all(&user_files).unwrap();
        let user_report = user_files.join("cpu-apple-silicon-pipeline-results.md");
        std::fs::write(&user_report, "user-owned\n").unwrap();

        let completion = seed_at(
            &store,
            &data,
            &repo,
            DemoSelection {
                harness: "codex".into(),
                model: None,
                permission_mode: None,
                reasoning_level: None,
            },
        )
        .unwrap();

        assert_eq!(completion.project.slug, FALLBACK_PROJECT_SLUG);
        assert_eq!(
            std::fs::read_to_string(user_report).unwrap(),
            "user-owned\n"
        );
        assert!(data
            .join("files")
            .join(FALLBACK_PROJECT_SLUG)
            .join("cpu-apple-silicon-pipeline-results.md")
            .is_file());
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn repository_commit_ids_are_deterministic() {
        let root = std::env::temp_dir().join(format!("orx-demo-test-{}", uuid::Uuid::new_v4()));
        let first = root.join("first");
        let second = root.join("second");
        let first_sha = install_repository(&first, &root.join("first.git")).unwrap();
        let second_sha = install_repository(&second, &root.join("second.git")).unwrap();
        assert_eq!(first_sha, second_sha);
        assert_eq!(
            git(&first, &["rev-parse", "refs/heads/main"]).unwrap(),
            git(&second, &["rev-parse", "refs/heads/main"]).unwrap()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn moved_data_dir_repairs_the_local_origin() {
        let root = std::env::temp_dir().join(format!("orx-demo-test-{}", uuid::Uuid::new_v4()));
        let data = root.join("data");
        let moved = root.join("moved-data");
        let repo = root.join("cache/repos").join(OWNER).join(REPO);
        let store = Store::open_at(data.clone()).unwrap();
        seed_at(
            &store,
            &data,
            &repo,
            DemoSelection {
                harness: "codex".into(),
                model: None,
                permission_mode: None,
                reasoning_level: None,
            },
        )
        .unwrap();
        drop(store);
        std::fs::rename(&data, &moved).unwrap();

        repair_installed_origin_at(&moved, &repo).unwrap();

        assert_eq!(
            git(&repo, &["remote", "get-url", "origin"]).unwrap(),
            moved.join("demo-repos/nanochat.git").to_string_lossy()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cleared_cache_restores_from_local_origin_at_experiment_commit() {
        let root = std::env::temp_dir().join(format!("orx-demo-test-{}", uuid::Uuid::new_v4()));
        let data = root.join("data");
        let repo = root.join("cache/repos").join(OWNER).join(REPO);
        let worktrees = root.join("cache/worktrees").join(PROJECT_ID);
        let store = Store::open_at(data.clone()).unwrap();
        seed_at(
            &store,
            &data,
            &repo,
            DemoSelection {
                harness: "codex".into(),
                model: None,
                permission_mode: None,
                reasoning_level: None,
            },
        )
        .unwrap();
        std::fs::remove_dir_all(&worktrees).unwrap();
        std::fs::remove_dir_all(&repo).unwrap();

        crate::local::git::restore_local_repository(
            &repo,
            &data.join("demo-repos/nanochat.git"),
            "main",
        )
        .unwrap();
        assert_eq!(git(&repo, &["branch", "--show-current"]).unwrap(), "main");
        assert_eq!(
            git(&repo, &["rev-parse", "refs/heads/main"]).unwrap(),
            BASELINE_SHA
        );
        assert_eq!(
            git(&repo, &["rev-parse", &format!("refs/heads/{BRANCH}")]).unwrap(),
            EXPERIMENT_SHA
        );
        for session_id in [SESSION_ID, FIGURE_SESSION_ID, LITERATURE_SESSION_ID] {
            let worktree = worktrees.join(session_id);
            crate::local::git::ensure_session_worktree_in(
                &repo, &worktree, OWNER, REPO, "main", session_id,
            )
            .unwrap();
            assert_eq!(
                git(&worktree, &["rev-parse", "HEAD"]).unwrap(),
                EXPERIMENT_SHA
            );
        }
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_reserved_repository_is_not_overwritten() {
        let root = std::env::temp_dir().join(format!("orx-demo-test-{}", uuid::Uuid::new_v4()));
        let repo = root.join("nanochat");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("keep-me"), "user data").unwrap();
        let error = install_repository(&repo, &root.join("origin.git")).unwrap_err();
        assert!(error
            .to_string()
            .contains("move it aside and retry onboarding"));
        assert_eq!(
            std::fs::read_to_string(repo.join("keep-me")).unwrap(),
            "user data"
        );
        assert!(!repo.join(".git").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
