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
//!   `.agents/skills`, `.cursor/skills`), so the session's own agent auto-discovers them and never
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

/// One embedded skill package: its complete `SKILL.md` and every other file in
/// its directory, loaded lazily by the agent.
pub struct AgentSkill {
    pub name: &'static str,
    pub content: &'static str,
    pub resources: &'static [AgentSkillResource],
}

impl AgentSkill {
    /// The frontmatter `description`: all a harness shows the agent until it loads the skill.
    pub fn description(&self) -> String {
        super::user_skills::parse_frontmatter(self.content)
            .map(|frontmatter| frontmatter.description)
            .unwrap_or_default()
    }
}

/// Every directory under `agent-skills/` with a `SKILL.md`, in name order.
static BUNDLED: &[AgentSkill] = include!(concat!(env!("OUT_DIR"), "/agent_skills.rs"));

pub const RETIRED_SKILL_NAMES: &[&str] = &[
    "orx-lit",
    "orx-compute-k8s",
    "orx-evidence",
    "orx-logs",
    "orx-instances",
];

/// Project onboarding, which a session that already has a project never needs.
const FULL_ONLY: &[&str] = &["orx-create"];

/// Which module set to serve. Both sets use the same canonical module bodies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillSet {
    /// Skills installed into an active `orx up` project session.
    Local,
    /// The complete set, including project onboarding guidance.
    Full,
}

/// The modules for a given set, in name order.
pub fn skills(set: SkillSet) -> Vec<&'static AgentSkill> {
    BUNDLED
        .iter()
        .filter(|skill| set == SkillSet::Full || !FULL_ONLY.contains(&skill.name))
        .collect()
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

    /// Harnesses list every description in every turn's context, so each names
    /// only when to load the skill.
    #[test]
    fn frontmatter_names_the_directory_and_quotes_a_short_description() {
        for s in skills(SkillSet::Full) {
            let mut lines = s.content.lines();
            assert_eq!(lines.next(), Some("---"), "{} missing opening ---", s.name);
            assert_eq!(lines.next(), Some(format!("name: {}", s.name).as_str()));

            // A JSON-quoted scalar is valid YAML whatever it contains, such as `: `.
            let value = lines
                .next()
                .and_then(|line| line.strip_prefix("description: "))
                .unwrap_or_else(|| panic!("{} description frontmatter shape", s.name));
            let description: String = serde_json::from_str(value)
                .unwrap_or_else(|e| panic!("{} description is not a quoted scalar: {e}", s.name));
            assert_eq!(description, s.description());
            let len = description.chars().count();
            assert!(
                (1..=200).contains(&len),
                "{} description is {len} chars",
                s.name
            );

            assert_eq!(lines.next(), Some("---"), "{} missing closing ---", s.name);
            let body = s.content.split_once("\n---\n\n").map(|(_, body)| body);
            assert!(
                body.is_some_and(|body| !body.trim().is_empty()),
                "{} has an empty body",
                s.name
            );
        }
    }

    /// A resource nothing names is one no agent opens.
    #[test]
    fn every_resource_is_named_by_its_skill() {
        for s in skills(SkillSet::Full) {
            for resource in s.resources {
                assert!(
                    std::iter::once(s.content)
                        .chain(s.resources.iter().map(|other| other.content))
                        .any(|text| text.contains(resource.path)),
                    "nothing in {} names {}",
                    s.name,
                    resource.path
                );
            }
        }
    }

    #[test]
    fn python_interval_merges_sparse_metrics_and_resumed_rank_and_errors() {
        use crate::process::command;
        if command("python3").arg("--version").output().is_err() {
            crate::config::skip_without_tool("python3 is unavailable");
            return;
        }
        let dir = crate::local::git::TemporaryDirectory::new("orx-python-interval").unwrap();
        let (_, resource) = find_resource("compute/assets/orx_run.py", SkillSet::Local).unwrap();
        std::fs::write(dir.path().join("orx_run.py"), resource.content).unwrap();
        let code = r#"import orx_run,json
clock=[0.0]
orx_run.time.monotonic=lambda:clock[0]
r=orx_run.progress(total=5000,start=3000,interval=.001)
for i in range(1000):
    r.step(3000+i, loss=i, val_loss=i if i%100==0 else None)
    clock[0]+=.0001
try:
    r.step(4000, bad=object())
    raise AssertionError('serialization error was delayed')
except TypeError:
    pass
class Scalar:
    def item(self): return float('nan')
try:
    import numpy
    nonfinite=numpy.float32(float('nan'))
except ImportError:
    nonfinite=Scalar()
r.step(4000, bad=nonfinite)
r.flush()
"#;
        let output = command("python3")
            .args(["-c", code])
            .current_dir(dir.path())
            .env("RANK", "")
            .env("SLURM_PROCID", "0")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let lines: Vec<serde_json::Value> = String::from_utf8(output.stdout.clone())
            .unwrap()
            .lines()
            .filter(|line| !line.contains(":NaN"))
            .map(|line| serde_json::from_str(line.strip_prefix("::orx:: ").unwrap()).unwrap())
            .collect();
        assert_eq!(lines[0]["step"], 3000);
        let values: std::collections::BTreeSet<_> = lines
            .iter()
            .filter_map(|v| v["m"]["val_loss"].as_u64())
            .collect();
        assert_eq!(values, (0..10).map(|i| i * 100).collect());
        let mut folded = orx_signals::Snapshot::new("r1", "1-100", "g1");
        folded.feed(&output.stdout, true, 1, false).unwrap();
        assert_eq!(folded.latest["bad"], "NaN");
        let muted = command("python3")
            .args([
                "-c",
                "import orx_run; orx_run.progress(start=3).step(4,x=1)",
            ])
            .current_dir(dir.path())
            .env_remove("RANK")
            .env("SLURM_PROCID", "1")
            .output()
            .unwrap();
        assert!(muted.status.success());
        assert!(muted.stdout.is_empty());
        let other_launcher = command("python3")
            .args([
                "-c",
                "import orx_run; orx_run.progress(start=3).step(4,x=1)",
            ])
            .current_dir(dir.path())
            .env_remove("RANK")
            .env_remove("SLURM_PROCID")
            .env("OMPI_COMM_WORLD_RANK", "1")
            .output()
            .unwrap();
        assert!(
            !other_launcher.stdout.is_empty(),
            "only rank=, RANK or SLURM_PROCID name a process's rank"
        );
    }

    /// What the fold would drop, the helper still prints and says once on stderr, without raising
    /// inside the training it runs in.
    #[test]
    fn python_warns_once_of_each_report_the_fold_drops() {
        use crate::process::command;
        if command("python3").arg("--version").output().is_err() {
            crate::config::skip_without_tool("python3 is unavailable");
            return;
        }
        let dir = crate::local::git::TemporaryDirectory::new("orx-python-dropped").unwrap();
        let (_, resource) = find_resource("compute/assets/orx_run.py", SkillSet::Local).unwrap();
        std::fs::write(dir.path().join("orx_run.py"), resource.content).unwrap();
        let code = r#"import orx_run
orx_run.plan(["setup", {"phase": "train", "of": 0}])
orx_run.plan(["stage%d" % i for i in range(65)])
run = orx_run.progress(phase="x" * 129, total=10, interval=0)
for step in range(3):
    run.step(step, loss=0.5, ok=True, hist=[1, 2], note="y" * 1025, **{"val/loss": 1.0})
run.row("bad-table", task="a")
run.step(3, odd="\udc80")
"#;
        let run = |rank: &str| {
            command("python3")
                .args(["-c", code])
                .current_dir(dir.path())
                .env("RANK", rank)
                .env_remove("SLURM_PROCID")
                .output()
                .unwrap()
        };

        let output = run("0");

        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let warnings = String::from_utf8_lossy(&output.stderr);
        for dropped in [
            "the whole plan for the total 0",
            "the whole plan: it lists 65 stages",
            "the stage name 'xxx",
            "ok=True",
            "hist, a list",
            "note: its text is over 1024 bytes",
            "'val/loss'",
            "the table 'bad-table'",
        ] {
            assert_eq!(
                warnings
                    .lines()
                    .filter(|line| line.contains(dropped))
                    .count(),
                1,
                "{dropped:?} in {warnings}"
            );
        }
        let mut folded = orx_signals::Snapshot::new("r1", "1-100", "g1");
        folded.feed(&output.stdout, true, 1, false).unwrap();
        assert_eq!(folded.latest["loss"], 0.5);
        assert!(folded.plan.is_none(), "the fold drops both plans");
        let other = run("1");
        assert!(other.status.success());
        assert!(other.stdout.is_empty() && other.stderr.is_empty());
    }

    #[test]
    fn python_plan_declares_every_stage_and_the_fold_keeps_it() {
        use crate::process::command;
        if command("python3").arg("--version").output().is_err() {
            crate::config::skip_without_tool("python3 is unavailable");
            return;
        }
        let dir = crate::local::git::TemporaryDirectory::new("orx-python-plan").unwrap();
        let (_, resource) = find_resource("compute/assets/orx_run.py", SkillSet::Local).unwrap();
        std::fs::write(dir.path().join("orx_run.py"), resource.content).unwrap();
        let code = r#"import orx_run
orx_run.plan(["install", {"phase": "train", "of": 5000, "unit": "steps"}, "eval", "train"])
orx_run.progress(phase="install", total=3, unit="packages").step(1)
for bad in (["a", "a"], [{"of": 3}], [""]):
    try:
        orx_run.plan(bad)
        raise AssertionError(f"{bad} was accepted")
    except ValueError:
        pass
"#;
        let output = command("python3")
            .args(["-c", code])
            .current_dir(dir.path())
            .env_remove("RANK")
            .env_remove("SLURM_PROCID")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut folded = orx_signals::Snapshot::new("r1", "1-100", "g1");
        folded.feed(&output.stdout, true, 1, false).unwrap();
        let plan = folded.plan.expect("the fold keeps the plan");
        let names: Vec<_> = plan.iter().map(|stage| stage.phase.as_str()).collect();
        assert_eq!(names, ["install", "train", "eval", "train"]);
        let first = &folded.phases[0];
        assert_eq!(
            (first.name.as_str(), first.of, first.unit.as_deref()),
            ("install", Some(3.0), Some("packages")),
            "progress() names the stage its first report starts"
        );
        assert_eq!(
            (
                plan[1].of.as_ref().and_then(serde_json::Number::as_f64),
                plan[1].unit.as_deref()
            ),
            (Some(5000.0), Some("steps"))
        );
        assert!(
            folded.diagnostics.counts.is_empty(),
            "{:?}",
            folded.diagnostics
        );
    }

    /// The documented resume, from a checkpoint older than the last report, and a second
    /// `progress()` with its own total, as the helper writes them.
    #[test]
    fn python_relaunch_from_an_older_checkpoint_folds_from_where_it_resumed() {
        use crate::process::command;
        if command("python3").arg("--version").output().is_err() {
            crate::config::skip_without_tool("python3 is unavailable");
            return;
        }
        let dir = crate::local::git::TemporaryDirectory::new("orx-python-relaunch").unwrap();
        let (_, resource) = find_resource("compute/assets/orx_run.py", SkillSet::Local).unwrap();
        std::fs::write(dir.path().join("orx_run.py"), resource.content).unwrap();
        let process = |start: u32, end: u32, offset: f64, then: &str| {
            let code = format!(
                r#"import orx_run
clock=[0.0]
orx_run.time.monotonic=lambda:clock[0]
r=orx_run.progress(total=2000,start={start},interval=0)
for step in range({start}+1,{end}+1):
    clock[0]+=0.5
    r.step(step, loss=step+{offset})
{then}"#
            );
            let output = command("python3")
                .args(["-c", &code])
                .current_dir(dir.path())
                .env("RANK", "0")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            output.stdout
        };
        let mut log = process(0, 1200, 0.0, "");
        let second = log.len();
        log.extend(process(
            1000,
            1400,
            0.5,
            "e=orx_run.progress(total=50,interval=0)\nfor step in range(1,8):\n    clock[0]+=0.5\n    e.step(step, score=step)",
        ));
        let mut fold = orx_signals::Snapshot::new("r1", "1-1", "g1");
        fold.feed(&log[..second], false, 1000, true).unwrap();
        let first_report = log[second..].iter().position(|&b| b == b'\n').unwrap() + 1;
        fold.feed(&log[second..second + first_report], false, 2000, true)
            .unwrap();
        assert_eq!(fold.progress().unwrap().done, Some(1000.));
        fold.feed(&log[second + first_report..], false, 3000, true)
            .unwrap();

        let progress = fold.progress().unwrap();
        assert_eq!((progress.done, progress.of), (Some(7.), Some(50.)));
        let loss = &fold.series["loss"];
        assert_eq!(loss.len(), 1400);
        assert!(loss.windows(2).all(|pair| pair[0].0 < pair[1].0));
        assert_eq!(loss[1099], (1100., Some(1100.5)));
        assert!(
            fold.diagnostics.counts.is_empty(),
            "a relaunch and a second loop are normal: {:?}",
            fold.diagnostics.counts
        );
    }

    /// Each stage's script is its own process; one that does not restate a total keeps the one
    /// its phase has, whether the shell declared it or an earlier script did.
    #[test]
    fn python_in_a_later_process_keeps_its_phase_total() {
        use crate::process::command;
        if command("python3").arg("--version").output().is_err() {
            crate::config::skip_without_tool("python3 is unavailable");
            return;
        }
        let dir = crate::local::git::TemporaryDirectory::new("orx-python-stages").unwrap();
        let (_, resource) = find_resource("compute/assets/orx_run.py", SkillSet::Local).unwrap();
        std::fs::write(dir.path().join("orx_run.py"), resource.content).unwrap();
        let python = |code: &str| {
            let code = format!(
                "import orx_run\nclock=[0.0]\norx_run.time.monotonic=lambda:clock[0]\n{code}"
            );
            let output = command("python3")
                .args(["-c", &code])
                .current_dir(dir.path())
                .env("RANK", "0")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            output.stdout
        };
        let steps = |total: &str, start: u32, end: u32| {
            format!("r=orx_run.progress({total}start={start},interval=0)\nfor step in range({start}+1,{end}+1):\n    clock[0]+=0.5\n    r.step(step, loss=step)")
        };
        let mut log = b"::orx:: {\"phase\":\"train\"}\n".to_vec();
        log.extend(python(&steps("total=100,", 0, 50)));
        log.extend(python(&steps("", 50, 60)));
        log.extend(b"::orx:: {\"phase\":\"eval\",\"step\":0,\"of\":22,\"unit\":\"tasks\"}\n");
        log.extend(python(
            "r=orx_run.progress(interval=0)\nclock[0]+=1\nr.step(3, score=1)",
        ));

        let mut fold = orx_signals::Snapshot::new("r1", "1-1", "g1");
        fold.feed(&log, true, 1000, false).unwrap();
        let [train, eval] = &fold.phases[..] else {
            panic!("{:?}", fold.phases);
        };
        assert_eq!((train.done, train.of), (Some(60.), Some(100.)));
        assert_eq!(
            (eval.done, eval.of, eval.unit.as_deref()),
            (Some(3.), Some(22.), Some("tasks"))
        );
        assert_eq!(fold.series["loss"].len(), 60);
        assert!(
            fold.diagnostics.counts.is_empty(),
            "{:?}",
            fold.diagnostics.counts
        );
    }

    #[test]
    fn python_progress_is_vendored_rank_filtered_and_foldable() {
        use crate::process::command;
        if command("python3").arg("--version").output().is_err() {
            crate::config::skip_without_tool("python3 is unavailable");
            return;
        }
        let dir = std::env::temp_dir().join(format!("orx-progress-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let (_, resource) = find_resource("compute/assets/orx_run.py", SkillSet::Local).unwrap();
        std::fs::write(dir.join("orx_run.py"), resource.content).unwrap();
        let code = r#"import orx_run as orx
r = orx.progress(total=10, interval=100000)
r.step(1, loss=3)
r.step(2, loss=float('nan'))
r.step(3, loss=2)
with r.phase('eval', total=2, unit='tasks') as p:
    p.step(1, score=0.5)
r.row('eval', task='sst2', score=0.9)
r.spend(1.25)
r.step(2, score=1)
"#;
        for (rank, enabled) in [("0", true), ("1", false)] {
            let output = command("python3")
                .args(["-c", code])
                .current_dir(&dir)
                .env("RANK", rank)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let mut fold = orx_signals::Snapshot::new("test", "1-2", "g1");
            fold.feed(&output.stdout, true, 1000, false).unwrap();
            if enabled {
                assert_eq!(fold.latest["score"], 1);
                assert_eq!(fold.spend, Some(1.25));
                assert_eq!(fold.rows["eval"].len(), 1);
                assert_eq!(fold.diagnostics.counts["non_finite"], 1);
                assert_eq!(fold.progress().unwrap().done, Some(2.0));
            } else {
                assert!(output.stdout.is_empty());
            }
        }
        let failed = command("python3")
            .args([
                "-c",
                r#"import sys
import orx_run as orx
class Broken:
    def write(self, text):
        raise OSError('full output device')
    def flush(self):
        pass
sys.stdout=Broken()
orx.progress(rank=0)
"#,
            ])
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(!failed.status.success());
        assert!(String::from_utf8_lossy(&failed.stderr).contains("full output device"));
        std::fs::remove_dir_all(dir).unwrap();
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
    fn figures_skill_ships_its_style_module() {
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
}
