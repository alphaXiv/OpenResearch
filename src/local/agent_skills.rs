//! Native modular agent skills for `orx`.
//!
//! The monolithic `orx skill` overview (repo-root `SKILL.md`) is factored into a
//! handful of focused modules that live as complete skill packages in the repo
//! `agent-skills/` directory (`agent-skills/<name>/SKILL.md` plus optional
//! resources), readable as-is on GitHub, embedded in the binary, and installed
//! verbatim. Two consumers use them:
//!
//! * **Local `orx up` sessions** get the [`SkillSet::Local`] modules written as
//!   native `SKILL.md` skill dirs *into the session worktree* — fresh on every
//!   turn, right beside the playbook (see [`ensure_session_skills`]). The harness
//!   picks the skills subdir (`.claude/skills`, `.opencode/skills`,
//!   `.agents/skills`), so the session's own agent auto-discovers them and never
//!   sees drift.
//! * **`orx skill <name>`** resolves a bundled module (with or without the
//!   `orx-` prefix) and prints it; `<name>/<resource>` prints one of its bundled
//!   references. The no-arg overview lists the top-level set.
//!   `orx install-skills --full` writes the Full set into an agent's global
//!   skills dir.
//!
//! Every module has one canonical `SKILL.md`. The Local set omits onboarding
//! because a session already has a project; the Full set includes it. The
//! `orx-` prefix on every dir name makes modules unmistakable in a skill list.

use std::path::Path;

use crate::error::{anyhow, Result};

/// One file bundled inside a skill package, relative to the skill directory.
pub struct AgentSkillResource {
    pub path: &'static str,
    pub content: &'static str,
}

/// One embedded skill package: its public metadata, complete `SKILL.md`, and
/// optional lazily loaded resources.
pub struct AgentSkill {
    pub name: &'static str,
    pub description: &'static str,
    pub content: &'static str,
    pub resources: &'static [AgentSkillResource],
}

pub const RETIRED_SKILL_NAMES: &[&str] = &["orx-lit", "orx-compute-k8s"];

/// Which module set to serve. Both sets use the same canonical module bodies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillSet {
    /// Skills installed into an active `orx up` project session.
    Local,
    /// The complete set, including project onboarding guidance.
    Full,
}

// --- Module files (embedded verbatim from the repo `agent-skills/` dir) ------

const COMPUTE: &str = include_str!("../../agent-skills/orx-compute/SKILL.md");
const COMPUTE_RESOURCES: &[AgentSkillResource] = &[
    AgentSkillResource {
        path: "references/hf.md",
        content: include_str!("../../agent-skills/orx-compute/references/hf.md"),
    },
    AgentSkillResource {
        path: "references/modal.md",
        content: include_str!("../../agent-skills/orx-compute/references/modal.md"),
    },
    AgentSkillResource {
        path: "references/k8s.md",
        content: include_str!("../../agent-skills/orx-compute/references/k8s.md"),
    },
    AgentSkillResource {
        path: "references/ssh.md",
        content: include_str!("../../agent-skills/orx-compute/references/ssh.md"),
    },
    AgentSkillResource {
        path: "references/slurm.md",
        content: include_str!("../../agent-skills/orx-compute/references/slurm.md"),
    },
    AgentSkillResource {
        path: "references/ray.md",
        content: include_str!("../../agent-skills/orx-compute/references/ray.md"),
    },
    AgentSkillResource {
        path: "references/openresearch.md",
        content: include_str!("../../agent-skills/orx-compute/references/openresearch.md"),
    },
    AgentSkillResource {
        path: "references/local.md",
        content: include_str!("../../agent-skills/orx-compute/references/local.md"),
    },
    AgentSkillResource {
        path: "references/tinker.md",
        content: include_str!("../../agent-skills/orx-compute/references/tinker.md"),
    },
];
const EXPERIMENT_TREE: &str = include_str!("../../agent-skills/orx-experiment-tree/SKILL.md");
const GIT: &str = include_str!("../../agent-skills/orx-git/SKILL.md");
const AGENT_DELEGATION: &str = include_str!("../../agent-skills/orx-agent-delegation/SKILL.md");
const LIT: &str = include_str!("../../agent-skills/orx-lit-review/SKILL.md");
const CREATE: &str = include_str!("../../agent-skills/orx-create/SKILL.md");
const REPORTS: &str = include_str!("../../agent-skills/orx-reports/SKILL.md");
const EVIDENCE: &str = include_str!("../../agent-skills/orx-evidence/SKILL.md");
const PAPER: &str = include_str!("../../agent-skills/orx-paper/SKILL.md");
const INSTANCES: &str = include_str!("../../agent-skills/orx-instances/SKILL.md");
const FIGURES: &str = include_str!("../../agent-skills/orx-figures/SKILL.md");
const FIGURES_RESOURCES: &[AgentSkillResource] = &[
    AgentSkillResource {
        path: "references/curves.md",
        content: include_str!("../../agent-skills/orx-figures/references/curves.md"),
    },
    AgentSkillResource {
        path: "references/scaling.md",
        content: include_str!("../../agent-skills/orx-figures/references/scaling.md"),
    },
    AgentSkillResource {
        path: "references/comparison.md",
        content: include_str!("../../agent-skills/orx-figures/references/comparison.md"),
    },
    AgentSkillResource {
        path: "references/pareto.md",
        content: include_str!("../../agent-skills/orx-figures/references/pareto.md"),
    },
    AgentSkillResource {
        path: "references/matrix.md",
        content: include_str!("../../agent-skills/orx-figures/references/matrix.md"),
    },
    AgentSkillResource {
        path: "references/diagram.md",
        content: include_str!("../../agent-skills/orx-figures/references/diagram.md"),
    },
    AgentSkillResource {
        path: "assets/orx_figstyle.py",
        content: include_str!("../../agent-skills/orx-figures/assets/orx_figstyle.py"),
    },
    AgentSkillResource {
        path: "assets/orx-tikz-preamble.tex",
        content: include_str!("../../agent-skills/orx-figures/assets/orx-tikz-preamble.tex"),
    },
];

// Descriptions are the *trigger surface*: what the module covers plus explicit,
// liberal "Use when …" cues (false positives beat false negatives — an agent
// that loads a module needlessly wastes a little context; one that misses it
// works blind). Keep each ≤400 chars — Codex's ambient budget is ~8k across
// the whole set.

const D_COMPUTE: &str = "Launch and monitor experiment runs and route guidance for hf, modal, k8s/Kubernetes, ssh, slurm, ray, OpenResearch, Tinker, and local backends. Covers the fixed run contract, sizing, cancellation, and wait versus wake. Use before any launch or relaunch, when authoring a k8s manifest, choosing or switching compute, or handling an OOM, stall, or timeout; then read one backend reference.";
const D_EXPERIMENT_TREE: &str = "Plan and drive the experiment tree: first-launch setup, fixed run contract, frozen nodes, stacked-bush tree shape, branch/launch/wait/promote, repair limits, notes, and turn summaries. Use before creating or changing experiments, launching a first run, deciding what to try next, handling a completed run, or reporting experiment progress.";

const S_COMPUTE: AgentSkill = AgentSkill {
    name: "orx-compute",
    description: D_COMPUTE,
    content: COMPUTE,
    resources: COMPUTE_RESOURCES,
};
const S_EXPERIMENT_TREE: AgentSkill = AgentSkill {
    name: "orx-experiment-tree",
    description: D_EXPERIMENT_TREE,
    content: EXPERIMENT_TREE,
    resources: &[],
};
const S_GIT: AgentSkill = AgentSkill {
    name: "orx-git",
    description: "Read, edit, commit, and diff experiment code; coordinate shared worktrees and preserve frozen branch history. Use before branch work, when starting work alongside other sessions, comparing nodes, preparing a run, repairing a provisional node, or diagnosing stale code.",
    content: GIT,
    resources: &[],
};
const S_AGENT_DELEGATION: AgentSkill = AgentSkill {
    name: "orx-agent-delegation",
    description: "Delegate independent work to helper agent sessions with `orx agent spawn`: task selection, self-contained briefs, branch ownership, compute authorization, wakeups, and nesting or concurrency constraints. Use before spawning a helper or interpreting its result; do not delegate the literature retrieval loop.",
    content: AGENT_DELEGATION,
    resources: &[],
};
const S_LIT: AgentSkill = AgentSkill {
    name: "orx-lit-review",
    description: "Search and read research papers. The main agent calls alphaXiv, OpenAlex, and bioRxiv discovery primitives, ranks the combined candidates, and chooses sources for focused follow-ups. Use for literature reviews, related work, prior art, papers, authors, methods, benchmarks, or research claims; never delegate the retrieval loop to a sub-agent.",
    content: LIT,
    resources: &[],
};
const S_CREATE: AgentSkill = AgentSkill {
    name: "orx-create",
    description: "Initialize a project with `orx up` and add experiment nodes with `orx create-experiment`. Use when starting a project or experiment, when the tree is empty, or when choosing a baseline, parent, or run command.",
    content: CREATE,
    resources: &[],
};
const S_REPORTS: AgentSkill = AgentSkill {
    name: "orx-reports",
    description: "Write durable outputs into the artifacts directory. Use when a line of work concludes or the user asks for a write-up, summary, comparison, figures, or exported data.",
    content: REPORTS,
    resources: &[],
};
const S_PAPER: AgentSkill = AgentSkill {
    name: "orx-paper",
    description: "Draft an academic paper or preprint as LaTeX. Create a .tex file in the project working tree, where it renders for the user and compiles to PDF. Use for a paper, preprint, manuscript, arXiv or submission draft, or a section of one; generic reports and result summaries belong to `orx-reports`.",
    content: PAPER,
    resources: &[],
};
const S_FIGURES: AgentSkill = AgentSkill {
    name: "orx-figures",
    description: "Publication-quality figures in matplotlib or TikZ: learning curves, scaling laws, benchmark and ablation comparisons, Pareto trade-offs, heatmaps and confusion matrices, method diagrams. Covers the shared style module, sizing, uncertainty, and vector export. Use whenever you plot, chart, or visualize results, add a figure to a paper or report, or one looks unpolished; then read one reference.",
    content: FIGURES,
    resources: FIGURES_RESOURCES,
};
const S_EVIDENCE: AgentSkill = AgentSkill {
    name: "orx-evidence",
    description: "Prepare and inspect experiment run evidence: design stdout metrics and summaries, read persisted results with `orx logs`, and validate run-derived claims. Use before launching a run whose output must be judged, after a run finishes, or before analyzing or reporting run results.",
    content: EVIDENCE,
    resources: &[],
};
const S_INSTANCES: AgentSkill = AgentSkill {
    name: "orx-instances",
    description: "Create standalone OpenResearch compute instances with `orx instance create`. Use when the user wants a persistent machine for manual or ad-hoc work rather than an experiment run.",
    content: INSTANCES,
    resources: &[],
};

/// The modules for a given set, in a stable order. Full adds `create`; every
/// shared module uses the same canonical `SKILL.md`.
pub fn skills(set: SkillSet) -> Vec<&'static AgentSkill> {
    match set {
        SkillSet::Local => vec![
            &S_EXPERIMENT_TREE,
            &S_GIT,
            &S_AGENT_DELEGATION,
            &S_COMPUTE,
            &S_INSTANCES,
            &S_EVIDENCE,
            &S_REPORTS,
            &S_FIGURES,
            &S_PAPER,
            &S_LIT,
        ],
        SkillSet::Full => vec![
            &S_CREATE,
            &S_EXPERIMENT_TREE,
            &S_GIT,
            &S_AGENT_DELEGATION,
            &S_COMPUTE,
            &S_INSTANCES,
            &S_EVIDENCE,
            &S_REPORTS,
            &S_FIGURES,
            &S_PAPER,
            &S_LIT,
        ],
    }
}

/// Resolve a bundled skill by name within `set`, accepting both the public name
/// (`orx-compute`) and the bare form (`compute`). Returns `None` for an unknown
/// bundled name.
pub fn find(name: &str, set: SkillSet) -> Option<&'static AgentSkill> {
    let want = name.trim();
    skills(set)
        .into_iter()
        .find(|s| s.name == want || s.name.strip_prefix("orx-") == Some(want))
}

/// Resolve a lazily loaded skill resource. `compute/hf` is shorthand for
/// `compute/references/hf.md`; exact relative paths are also accepted.
pub fn find_resource(
    path: &str,
    set: SkillSet,
) -> Option<(&'static AgentSkill, &'static AgentSkillResource)> {
    let (skill_name, requested) = path.trim().split_once('/')?;
    let skill = find(skill_name, set)?;
    let exact = requested.trim();
    let shorthand = format!("references/{exact}.md");
    skill
        .resources
        .iter()
        .find(|resource| resource.path == exact || resource.path == shorthand)
        .map(|resource| (skill, resource))
}

/// Write the [`SkillSet::Local`] modules as `<worktree>/<skills_dir_rel>/<name>/SKILL.md`,
/// overwriting every file on every call (same freshness semantics as the
/// playbook — zero drift). Returns `Err` on the first write failure; the caller
/// treats it like a playbook-write error.
pub fn ensure_session_skills(worktree: &Path, skills_dir_rel: &str) -> Result<()> {
    let base = worktree.join(skills_dir_rel);
    for name in RETIRED_SKILL_NAMES {
        let dir = base.join(name);
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(anyhow!("Could not remove {}: {}", dir.display(), error));
            }
        }
    }
    for skill in skills(SkillSet::Local) {
        let dir = base.join(skill.name);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .map_err(|e| anyhow!("Could not refresh {}: {}", dir.display(), e))?;
        }
        std::fs::create_dir_all(&dir)
            .map_err(|e| anyhow!("Could not create {}: {}", dir.display(), e))?;
        let path = dir.join("SKILL.md");
        std::fs::write(&path, skill.content)
            .map_err(|e| anyhow!("Could not write {}: {}", path.display(), e))?;
        for resource in skill.resources {
            let resource_path = dir.join(resource.path);
            let parent = resource_path
                .parent()
                .ok_or_else(|| anyhow!("Invalid skill resource path {}", resource.path))?;
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow!("Could not create {}: {}", parent.display(), e))?;
            std::fs::write(&resource_path, resource.content)
                .map_err(|e| anyhow!("Could not write {}: {}", resource_path.display(), e))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn is_valid_name(name: &str) -> bool {
        // ^[a-z0-9]+(-[a-z0-9]+)*$
        !name.is_empty()
            && name.split('-').all(|seg| {
                !seg.is_empty()
                    && seg
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            })
    }

    #[test]
    fn names_are_valid_unique_and_prefixed() {
        for set in [SkillSet::Local, SkillSet::Full] {
            let mut seen = HashSet::new();
            for s in skills(set) {
                assert!(
                    is_valid_name(s.name),
                    "{:?}: invalid name {:?}",
                    set,
                    s.name
                );
                assert!(
                    s.name.starts_with("orx-"),
                    "{:?}: name {:?} not orx- prefixed",
                    set,
                    s.name
                );
                assert!(
                    seen.insert(s.name),
                    "{:?}: duplicate name {:?}",
                    set,
                    s.name
                );
            }
        }
    }

    #[test]
    fn retired_names_are_prefixed_and_not_current() {
        for name in RETIRED_SKILL_NAMES {
            assert!(
                name.starts_with("orx-"),
                "unsafe retired skill name: {name}"
            );
            for set in [SkillSet::Local, SkillSet::Full] {
                assert!(
                    skills(set).iter().all(|skill| skill.name != *name),
                    "{name} is both retired and current"
                );
            }
        }
    }

    #[test]
    fn descriptions_are_within_bounds() {
        for set in [SkillSet::Local, SkillSet::Full] {
            for s in skills(set) {
                let len = s.description.chars().count();
                assert!(
                    (1..=400).contains(&len),
                    "{:?}: {} description is {} chars (want 1..=400)",
                    set,
                    s.name,
                    len
                );
            }
        }
    }

    #[test]
    fn file_frontmatter_matches_code_and_is_valid_yaml() {
        // The skill files under `agent-skills/` are the literal installed
        // artifacts (embedded verbatim), so their frontmatter must agree with
        // the code's name/description — this is the drift guard between the
        // GitHub-readable files and the indexes generated from the consts.
        for set in [SkillSet::Local, SkillSet::Full] {
            for s in skills(set) {
                let mut lines = s.content.lines();
                assert_eq!(lines.next(), Some("---"), "{} missing opening ---", s.name);
                let name_line = lines.next().unwrap_or_default();
                let desc_line = lines.next().unwrap_or_default();
                assert_eq!(
                    name_line,
                    format!("name: {}", s.name),
                    "{} name frontmatter",
                    s.name
                );

                // The description value must be YAML-safe. The files carry it
                // as a JSON-quoted scalar; strip `description: ` and JSON-decode
                // — the round-trip proves the quoting is well-formed and that
                // the file agrees with the code's description. A bare
                // (unquoted) value containing `: ` — the bug this guards —
                // would not JSON-decode.
                let value = desc_line
                    .strip_prefix("description: ")
                    .unwrap_or_else(|| panic!("{} description frontmatter shape", s.name));
                let decoded: String = serde_json::from_str(value).unwrap_or_else(|e| {
                    panic!("{} description is not a quoted scalar: {e}", s.name)
                });
                assert_eq!(decoded, s.description, "{} description round-trip", s.name);
                // A quoted scalar is a single physical line — no embedded newline.
                assert!(
                    !s.description.contains('\n'),
                    "{} description has a newline",
                    s.name
                );

                assert_eq!(lines.next(), Some("---"), "{} missing closing ---", s.name);
                // A non-empty body follows the closing frontmatter fence
                // (`\n---\n\n` separates the frontmatter block from the body).
                let body = s
                    .content
                    .split_once("\n---\n\n")
                    .map(|(_, body)| body)
                    .unwrap_or("");
                assert!(!body.trim().is_empty(), "{} has an empty body", s.name);
            }
        }
    }

    #[test]
    fn find_resolves_prefixed_and_bare() {
        for set in [SkillSet::Local, SkillSet::Full] {
            assert_eq!(
                find("orx-compute", set).map(|s| s.name),
                Some("orx-compute")
            );
            assert_eq!(find("compute", set).map(|s| s.name), Some("orx-compute"));
            assert!(find("does-not-exist", set).is_none());
            assert!(find("project-query", set).is_none());
        }
        // `orx-create` is Full-only — a local session has no create surface.
        assert_eq!(
            find("orx-create", SkillSet::Full).map(|s| s.name),
            Some("orx-create")
        );
        assert_eq!(
            find("create", SkillSet::Full).map(|s| s.name),
            Some("orx-create")
        );
        assert!(find("orx-create", SkillSet::Local).is_none());
        assert!(find("orx-compute-k8s", SkillSet::Full).is_none());
        let (_, hf) = find_resource("compute/hf", SkillSet::Local).unwrap();
        assert_eq!(hf.path, "references/hf.md");
        let (_, k8s) = find_resource("orx-compute/references/k8s.md", SkillSet::Local).unwrap();
        assert_eq!(k8s.path, "references/k8s.md");
        let (_, tinker) = find_resource("compute/tinker", SkillSet::Local).unwrap();
        assert_eq!(tinker.path, "references/tinker.md");
        assert!(find_resource("compute/unknown", SkillSet::Local).is_none());
    }

    #[test]
    fn shared_skills_use_one_canonical_body() {
        for name in [
            "experiment-tree",
            "git",
            "agent-delegation",
            "compute",
            "instances",
            "evidence",
            "reports",
        ] {
            let local = find(name, SkillSet::Local).expect("local skill");
            let full = find(name, SkillSet::Full).expect("full skill");
            assert_eq!(local.content, full.content, "{name} body");
            let local_resources: Vec<_> = local
                .resources
                .iter()
                .map(|resource| (resource.path, resource.content))
                .collect();
            let full_resources: Vec<_> = full
                .resources
                .iter()
                .map(|resource| (resource.path, resource.content))
                .collect();
            assert_eq!(local_resources, full_resources, "{name} resources");
        }
    }

    #[test]
    fn evidence_and_reports_have_distinct_ownership() {
        assert!(EVIDENCE.contains("Validate before reporting"));
        assert!(EVIDENCE.contains("Truncated output is not evidence of absence"));
        assert!(!EVIDENCE.contains("<file path="));
        assert!(REPORTS.contains("evidence-and-links contract"));
        assert!(REPORTS.contains("Load `orx-evidence`"));
    }

    #[test]
    fn paper_skill_writes_a_compilable_tex_and_cites_it() {
        assert!(PAPER.contains("keep the `.tex` out of the artifacts"));
        assert!(PAPER.contains("evidence-and-links contract"));
        assert!(PAPER.contains("`orx-lit-review` workflow"));
        assert!(PAPER.contains("`orx discover`"));
        // The trap that makes a draft fail to compile rather than look wrong.
        assert!(PAPER.contains("Every environment needs the package that defines it"));
        assert!(PAPER.contains("\\newtheorem"));
        // \citet without natbib is a hard build failure the preview never shows.
        assert!(PAPER.contains("natbib"));
        // The engine a document needs travels in the file, not in a setting.
        assert!(PAPER.contains("% !TeX program"));
        for set in [SkillSet::Local, SkillSet::Full] {
            assert_eq!(find("paper", set).map(|s| s.name), Some("orx-paper"));
        }
    }

    #[test]
    fn ensure_session_skills_writes_local_set_idempotently() {
        let tmp = std::env::temp_dir().join(format!(
            "orx-agent-skills-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let rel = ".claude/skills";
        for name in RETIRED_SKILL_NAMES {
            let retired = tmp.join(rel).join(name);
            std::fs::create_dir_all(&retired).unwrap();
            std::fs::write(retired.join("SKILL.md"), "stale").unwrap();
        }
        ensure_session_skills(&tmp, rel).unwrap();
        for name in RETIRED_SKILL_NAMES {
            assert!(
                !tmp.join(rel).join(name).exists(),
                "retired bundled skill was pruned"
            );
        }

        let base = tmp.join(rel);
        let expected: HashSet<&str> = skills(SkillSet::Local).iter().map(|s| s.name).collect();
        let got: HashSet<String> = std::fs::read_dir(&base)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        let got_refs: HashSet<&str> = got.iter().map(String::as_str).collect();
        assert_eq!(got_refs, expected, "wrote exactly the Local-set dirs");

        for s in skills(SkillSet::Local) {
            let path = base.join(s.name).join("SKILL.md");
            let content = std::fs::read_to_string(&path).unwrap();
            assert_eq!(content, s.content, "{} SKILL.md content", s.name);
            for resource in s.resources {
                let content =
                    std::fs::read_to_string(base.join(s.name).join(resource.path)).unwrap();
                assert_eq!(
                    content, resource.content,
                    "{} {} content",
                    s.name, resource.path
                );
            }
        }

        // Idempotent: a second call overwrites in place and changes nothing.
        ensure_session_skills(&tmp, rel).unwrap();
        let got2: HashSet<String> = std::fs::read_dir(&base)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(got2, got, "second call is idempotent");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn unified_git_and_compute_skills_are_written_to_sessions() {
        let tmp =
            std::env::temp_dir().join(format!("orx-unified-skills-test-{}", uuid::Uuid::new_v4()));
        let rel = ".agents/skills";
        ensure_session_skills(&tmp, rel).unwrap();
        let git = std::fs::read_to_string(tmp.join(rel).join("orx-git/SKILL.md")).unwrap();
        assert!(git.contains("never part of compute transport"));
        assert!(git.contains("do not push merely to launch compute"));
        let compute = std::fs::read_to_string(tmp.join(rel).join("orx-compute/SKILL.md")).unwrap();
        assert!(compute.contains("immutable source snapshot"));
        assert!(compute.contains("references/hf.md"));
        let hf =
            std::fs::read_to_string(tmp.join(rel).join("orx-compute/references/hf.md")).unwrap();
        assert!(hf.contains("Hugging Face Jobs"));
        let tinker =
            std::fs::read_to_string(tmp.join(rel).join("orx-compute/references/tinker.md"))
                .unwrap();
        assert!(tinker.contains("TINKER_API_KEY"));
        assert!(!tmp.join(rel).join("orx-compute-k8s").exists());
        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn figures_skill_ships_its_style_module_and_routes_by_figure_type() {
        // The references are worth nothing if the style module they all import
        // is not installed beside them, so pin both halves.
        let figures = find("figures", SkillSet::Local).expect("figures skill");
        let paths: HashSet<&str> = figures.resources.iter().map(|r| r.path).collect();
        for expected in [
            "references/curves.md",
            "references/scaling.md",
            "references/comparison.md",
            "references/pareto.md",
            "references/matrix.md",
            "references/diagram.md",
            "assets/orx_figstyle.py",
            "assets/orx-tikz-preamble.tex",
        ] {
            assert!(paths.contains(expected), "figures is missing {expected}");
            assert!(
                figures.content.contains(expected)
                    || figures
                        .resources
                        .iter()
                        .any(|r| r.content.contains(expected)),
                "nothing routes to {expected}"
            );
        }

        let style = find_resource("figures/assets/orx_figstyle.py", SkillSet::Local)
            .expect("style module")
            .1;
        // arXiv rejects Type 3 fonts; every figure inherits the fix from here.
        assert!(style.content.contains("\"pdf.fonttype\": 42"));
        // Every rule the audit enforces is one an agent would otherwise skip.
        for check in [
            "problems.append(f\"overlapping text:",
            "text runs off the canvas and will be clipped",
            "text below the 5pt floor",
            "duplicates the caption \u{2014} delete it",
        ] {
            assert!(
                style.content.contains(check),
                "audit lost its {check} check"
            );
        }
    }

    #[test]
    fn figures_skill_pairs_each_destination_with_its_citation_tag() {
        // A worktree figure cited with an `artifacts/` prefix resolves in
        // neither root and reaches the user as a dead chip.
        let figures = find("figures", SkillSet::Local).expect("figures skill");
        assert!(figures
            .content
            .contains(r#"<file path="figs/loss_curve.pdf" />"#));
        assert!(figures
            .content
            .contains(r#"<file path="artifacts/loss_curve.pdf" />"#));
        assert!(figures
            .content
            .contains("The tag must match the destination"));
        assert!(
            figures.content.contains("/tmp"),
            "must rule out scratch dirs"
        );
    }

    #[test]
    fn bundled_skills_avoid_openresearch_ui_navigation() {
        for set in [SkillSet::Local, SkillSet::Full] {
            for skill in skills(set) {
                crate::local::assert_agent_guidance_is_ui_agnostic(skill.name, skill.content);
                crate::local::assert_agent_guidance_is_ui_agnostic(skill.name, skill.description);
                for resource in skill.resources {
                    crate::local::assert_agent_guidance_is_ui_agnostic(
                        resource.path,
                        resource.content,
                    );
                }
            }
        }
    }
}
