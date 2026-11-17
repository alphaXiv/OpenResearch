use crate::engine::{estimate, monitor, view};
use crate::error::{anyhow, Result};
use crate::store::Store;

pub(super) fn run(store: &Store, args: &crate::ExpRunArgs) -> Result<()> {
    let experiment = crate::local::resolve::resolve_experiment(store, &args.exp_id)?;
    let settings = store.run_monitor_json(&experiment.id)?;
    let plan = view::stored_plan(&experiment.id, settings.as_deref())?;
    let project = store
        .get_local_project(&experiment.project_id)?
        .ok_or_else(|| anyhow!("Project {} not found.", experiment.project_id))?;
    let command = crate::local::experiments::effective_run_command(
        &experiment,
        project.run_command.as_deref(),
    )
    .ok_or_else(|| {
        anyhow!(
            "{}",
            crate::invocation::no_run_command(&experiment.project_id)
        )
    })?;
    if plan.is_empty() {
        return Err(anyhow!("Experiment {} has no stage plan. Set every stage with `orx exp view set {} --config plan.json`, then estimate again.", experiment.id, experiment.id));
    }
    let mut parent = None;
    if let Some(id) = &experiment.parent_experiment_id {
        let experiment = store
            .get_local_experiment(id)?
            .ok_or_else(|| anyhow!("Parent experiment {id} not found."))?;
        for run in store.list_runs_by_experiment(id)? {
            if run.status != "done" {
                continue;
            }
            let snapshot = monitor::checkpoint(store, &run.id).map_err(|error| {
                anyhow!("Cannot read parent run {}'s stage timings: {error:#}. Inspect it with `orx exp view check {}` before sizing this experiment.", run.id, run.id)
            })?;
            if let Some(snapshot) = snapshot {
                if !snapshot.phases.is_empty() {
                    parent = Some((experiment, run, snapshot));
                    break;
                }
            }
        }
    }
    println!("Command: {command}");
    match &parent {
        Some((experiment, run, _)) => println!("Parent timing: {} — {}", experiment.display_name(), run.id),
        None => println!("Parent timing: unavailable; record a completed parent run before sizing this experiment."),
    }
    println!(
        "{}",
        report(
            &plan,
            parent.as_ref().map(|(_, _, snapshot)| snapshot.as_ref())
        )
    );
    Ok(())
}

fn report(plan: &[orx_signals::Stage], parent: Option<&orx_signals::Snapshot>) -> String {
    let timed: Vec<_> = parent
        .into_iter()
        .map(|p| (p, true, estimate::TimedBy::Parent))
        .collect();
    let stages = estimate::stages(plan, None, false, &timed);
    let duration = |ms: f64| {
        let seconds = (ms / 1000.0).round() as i64;
        if seconds < 60 {
            format!("{seconds} s")
        } else if seconds % 60 == 0 {
            format!("{} min", seconds / 60)
        } else {
            format!("{} min {} s", seconds / 60, seconds % 60)
        }
    };
    let mut lines = Vec::new();
    let mut total = 0.0;
    let mut unknown = Vec::new();
    let mut occurrences = std::collections::HashMap::new();
    for stage in stages {
        let occurrence = occurrences.entry(stage.name.clone()).or_insert(0);
        let previous = parent.and_then(|p| {
            p.phases
                .iter()
                .filter(|p| p.name == stage.name)
                .nth(*occurrence)
        });
        *occurrence += 1;
        let timing = (|| {
            let prior = stage.prior.ok_or("no recorded parent duration")?;
            let parent_unit = previous.and_then(|p| p.unit.as_deref());
            let unit = stage.unit.as_deref();
            if !estimate::compatible_units(unit, parent_unit) && stage.of.is_some() {
                return Err("the parent counted a different unit");
            }
            let (ms, explanation) = match stage.of {
                Some(of) => {
                    let each = prior
                        .per_unit_ms
                        .filter(|each| *each > 0.0)
                        .ok_or("no completed parent count to scale")?;
                    let unit = unit.or(parent_unit).unwrap_or("units");
                    (
                        each * of,
                        format!("{of} {unit}; the parent took {} each", duration(each)),
                    )
                }
                None => (prior.ms, "(as the parent)".into()),
            };
            if !ms.is_finite() {
                return Err("scaled duration exceeds the supported range");
            }
            Ok((ms, explanation))
        })();
        match timing {
            Ok((ms, explanation)) => {
                total += ms;
                let join = if stage.of.is_some() { " — " } else { " " };
                lines.push(format!(
                    "{}: about {}{join}{explanation}",
                    stage.name,
                    duration(ms)
                ));
            }
            Err(reason) => {
                lines.push(format!("{}: unknown — {reason}", stage.name));
                unknown.push(stage.name);
            }
        }
    }
    lines.push(
        "Not included: queueing, provisioning, and changes in hardware or per-step work.".into(),
    );
    if unknown.is_empty() {
        lines.push(format!("Total: about {}", duration(total)));
    } else {
        lines.push(format!(
            "Total: at least {} ({} not timed)",
            duration(total),
            unknown.join(", ")
        ));
        lines.push(format!("This is a floor: {} are not timed. Tell the user the total is unknown or run a short calibration before promising a duration.", unknown.join(", ")));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parent() -> orx_signals::Snapshot {
        let mut snapshot = orx_signals::Snapshot::new("parent", "log", "generation");
        snapshot
            .feed(b"::orx:: {\"phase\":\"download\"}\n", false, 0, true)
            .unwrap();
        snapshot
            .feed(
                b"::orx:: {\"phase\":\"train\",\"step\":0,\"of\":200,\"unit\":\"steps\"}\n",
                false,
                60_000,
                true,
            )
            .unwrap();
        snapshot
            .feed(b"::orx:: {\"step\":200}\n", true, 660_000, true)
            .unwrap();
        snapshot
    }

    #[test]
    fn sizing_scales_steps_includes_setup_and_names_untimed_stages() {
        let plan = orx_signals::plan(
            &json!([
                {"phase":"download"}, {"phase":"train", "of":600, "unit":"steps"}, {"phase":"eval"}
            ]),
            false,
        )
        .unwrap();
        let text = report(&plan, Some(&parent()));
        assert!(
            text.contains("600 steps; the parent took 3 s each"),
            "{text}"
        );
        assert!(text.contains("download: about 1 min"), "{text}");
        assert!(text
            .lines()
            .last()
            .unwrap()
            .contains("This is a floor: eval are not timed."));
        assert!(text.lines().last().unwrap().contains("total is unknown"));
        assert!(
            text.contains("Total: at least 31 min (eval not timed)"),
            "{text}"
        );
        assert!(report(&plan[..2], Some(&parent())).contains("Total: about 31 min"));
        assert!(report(&plan, None).contains("download, train, eval not timed"));
    }

    #[test]
    fn mismatched_units_and_missing_counts_stay_untimed() {
        let mut recorded = parent();
        let plan = orx_signals::plan(
            &json!([
                {"phase":"train", "of":600, "unit":"epochs"}
            ]),
            false,
        )
        .unwrap();
        assert!(report(&plan, Some(&recorded)).contains("train not timed"));
        recorded.phases[1].done = None;
        let plan = orx_signals::plan(
            &json!([
                {"phase":"train", "of":600}
            ]),
            false,
        )
        .unwrap();
        assert!(report(&plan, Some(&recorded)).contains("train not timed"));
        assert!(report(&plan, Some(&parent())).contains("Total: about 30 min"));
    }
}
