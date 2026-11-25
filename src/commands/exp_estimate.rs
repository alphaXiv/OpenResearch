use std::time::Duration;

use crate::client::SandboxTarget;
use crate::engine::{estimate, monitor, view};
use crate::error::{anyhow, Result};
use crate::local::spend::cost;
use crate::store::{Per, Quote, Store};

pub(super) async fn run(store: &Store, mut args: crate::ExpRunArgs) -> Result<()> {
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
    crate::local::apply_compute_default(
        store,
        &experiment.project_id,
        &mut args.backend,
        &mut args.flavor,
    )?;
    let backend = args.backend.get_or_insert_with(|| "local".into()).clone();
    crate::compute::validate_run_args(&args)?;
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
    let flags = |backend: &str, flavor: Option<&str>| match flavor {
        Some(flavor) => format!("--backend {backend} --flavor {flavor}"),
        None => format!("--backend {backend}"),
    };
    let compute = flags(&backend, args.flavor.as_deref());
    let limit = crate::config::paced(Duration::from_secs(60));
    let quote = tokio::time::timeout(limit, price(&backend, &args))
        .await
        .unwrap_or_else(|_| Err(anyhow!("no price within 60 s")));
    println!("Command: {command}");
    match &parent {
        Some((experiment, run, _)) => {
            let launch = match store.engine_run_state(&run.id) {
                Ok(state) => state.and_then(|state| state.launch),
                Err(error) => {
                    println!(
                        "Parent hardware unreadable: could not read how {} launched: {error:#}",
                        run.id
                    );
                    None
                }
            };
            let ran = crate::store::public_backend(&run.backend_json, launch.as_ref())
                .unwrap_or_default();
            let gpu = launch.as_ref().and_then(|launch| launch.options["gpu"].as_str());
            let ran_on = match ran["kind"].as_str() {
                Some(kind) => flags(
                    kind.trim_end_matches("_job"),
                    ran["flavor"].as_str().or(gpu),
                ),
                None => "a backend orx could not read".into(),
            };
            println!(
                "Parent timing: {} — {}, on {ran_on}",
                experiment.display_name(),
                run.id
            );
        }
        None => println!("Parent timing: unavailable; record a completed parent run before sizing this experiment."),
    }
    println!(
        "{}",
        report(
            &plan,
            parent.as_ref().map(|(_, _, snapshot)| snapshot.as_ref()),
            &compute,
            &quote,
        )
    );
    Ok(())
}

/// What the run's compute charges for running time, priced before launch the way its backend
/// bills; None where orx has no price for the backend.
async fn price(backend: &str, args: &crate::ExpRunArgs) -> Result<Option<Quote>> {
    use crate::jobs::{huggingface as hf, modal, openresearch};

    let flavor = || {
        args.flavor
            .as_deref()
            .ok_or_else(|| anyhow!("pass the --flavor the run will use"))
    };
    let quote = match backend {
        "hf" => hf::hardware_price(&hf::endpoint(), flavor()?).await?,
        // Estimates are plan-mode reads: use Modal's interpreter as set up, never install one.
        "modal" => modal::hourly_price(&modal::resolve_flavor(flavor()?)).await?,
        "openresearch" => {
            let creds = crate::config::load_credentials()
                .await?
                .ok_or_else(|| anyhow!("OpenResearch requires `orx login`."))?;
            let target = openresearch::parse_flavor(
                flavor()?,
                args.disk.unwrap_or(100),
                args.provider.clone(),
            )?;
            // The offer a launch gets: the cheapest that fits, the way openresearch.sh picks it.
            let dollars = match target {
                SandboxTarget::New {
                    gpu,
                    gpu_count,
                    disk_gb,
                    provider,
                } => {
                    let offers = crate::client::list_catalog(&creds).await?.offers;
                    let cheapest = offers
                        .iter()
                        .filter(|offer| {
                            let disk_fits = offer.disk.sizable
                                || offer
                                    .disk
                                    .included_gb
                                    .is_some_and(|gb| gb >= disk_gb as f64);
                            let provider_fits = provider
                                .as_ref()
                                .is_none_or(|name| offer.provider.eq_ignore_ascii_case(name));
                            offer.gpu.eq_ignore_ascii_case(&gpu)
                                && offer.gpu_count == gpu_count
                                && disk_fits
                                && provider_fits
                        })
                        .map(|offer| offer.price_per_hour)
                        .filter(|dollars| dollars.is_finite() && *dollars >= 0.0)
                        .min_by(f64::total_cmp);
                    let (from, flag) = match &provider {
                        Some(name) => (format!(" from {name}"), format!(" --provider {name}")),
                        None => Default::default(),
                    };
                    cheapest.ok_or_else(|| anyhow!("OpenResearch has no {gpu_count}× {gpu} offer{from} with {disk_gb} GB of disk now; list them with `orx compute --gpu {gpu} --count {gpu_count}{flag}`"))?
                }
                SandboxTarget::NewCpu {
                    cpu_flavor,
                    vcpu_count,
                } => crate::client::list_cpu_catalog(&creds)
                    .await?
                    .offers
                    .into_iter()
                    .filter(|offer| {
                        offer.cpu_flavor == cpu_flavor && offer.vcpus == vcpu_count as f64
                    })
                    .map(|offer| offer.price_per_hour)
                    .filter(|dollars| dollars.is_finite() && *dollars >= 0.0)
                    .min_by(f64::total_cmp)
                    .ok_or_else(|| {
                        anyhow!("OpenResearch has no {cpu_flavor} offer with {vcpu_count} vCPUs now; list them with `orx compute --cpu`")
                    })?,
            };
            Quote {
                amount_micros: (dollars * 1e6).round() as u64,
                per: Per::Hour,
                currency: "USD".into(),
                source: "openresearch.sh".into(),
                quoted_at: crate::store::now_ms(),
            }
        }
        _ => return Ok(None),
    };
    Ok(Some(quote))
}

/// Dollars to the cent, or to two significant digits below one.
fn usd(micros: u64) -> String {
    let dollars = micros as f64 / 1e6;
    let decimals = if micros == 0 || dollars >= 0.01 {
        2
    } else {
        (1.0 - dollars.log10().floor()) as usize
    };
    format!("${dollars:.decimals$}")
}

fn report(
    plan: &[orx_signals::Stage],
    parent: Option<&orx_signals::Snapshot>,
    compute: &str,
    price: &Result<Option<Quote>>,
) -> String {
    let timed: Vec<_> = parent
        .into_iter()
        .map(|p| (p, true, estimate::TimedBy::Parent))
        .collect();
    let stages = estimate::stages(plan, None, false, &timed);
    let planned = stages.len();
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
            let prior = stage.prior.ok_or_else(|| {
                previous
                    .and_then(|phase| estimate::prior_scale(&stage, phase).err())
                    .unwrap_or("no recorded parent duration")
            })?;
            let parent_unit = previous.and_then(|p| p.unit.as_deref());
            let unit = stage.unit.as_deref();
            let explanation = match stage.of {
                Some(of) => {
                    prior
                        .per_unit_ms
                        .filter(|each| *each > 0.0)
                        .ok_or("no completed parent count to scale")?;
                    let each = prior.ms / of;
                    let unit = unit.or(parent_unit).unwrap_or("units");
                    let per_unit = if each < 1.0 {
                        "under 1 ms".to_owned()
                    } else if each < 1000.0 {
                        format!("{each:.0} ms")
                    } else {
                        duration(each)
                    };
                    format!("{of} {unit}; the parent took {per_unit} each")
                }
                None => "(as the parent)".into(),
            };
            if !prior.ms.is_finite() {
                return Err("scaled duration exceeds the supported range");
            }
            Ok((prior.ms, explanation))
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
    let none_timed = unknown.len() == planned;
    let bound = if unknown.is_empty() {
        lines.push(format!("Total: about {}", duration(total)));
        Some("about")
    } else if none_timed {
        lines.push(format!("Total: unknown ({} not timed)", unknown.join(", ")));
        None
    } else {
        lines.push(format!(
            "Total: at least {} ({} not timed)",
            duration(total),
            unknown.join(", ")
        ));
        Some("at least")
    };
    match price {
        Ok(Some(quote)) => {
            let hourly = usd(cost(quote, Per::Hour.millis() as i64));
            lines.push(format!(
                "Price: {hourly}/hr for {compute}, from {}",
                quote.source
            ));
            lines.push(match bound {
                Some(bound) => format!("Cost: {bound} {}", usd(cost(quote, total.round() as i64))),
                None => "Cost: unknown until a run times its stages".into(),
            });
        }
        Ok(None) => lines.push(format!("Cost: orx has no price for {compute}")),
        Err(error) => lines.push(format!("Cost: unknown for {compute}: {error:#}")),
    }
    if none_timed {
        lines.push("No parent timing covers these stages, so this run's duration and cost are unknown until it measures them. Tell the user so, with the hourly price when there is one, or run a short calibration before promising a duration or cost.".into());
    } else if !unknown.is_empty() {
        lines.push(format!("This is a floor: {} are not timed. Tell the user the total is unknown or run a short calibration before promising a duration or cost.", unknown.join(", ")));
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
        let text = report(&plan, Some(&parent()), "--backend local", &Ok(None));
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
        assert!(
            report(&plan[..2], Some(&parent()), "--backend local", &Ok(None))
                .contains("Total: about 31 min")
        );
        assert!(report(&plan, None, "--backend local", &Ok(None))
            .contains("download, train, eval not timed"));
    }

    #[test]
    fn sizing_prices_the_total_at_the_backend_rate() {
        let plan = orx_signals::plan(
            &json!([
                {"phase":"download"}, {"phase":"train", "of":600, "unit":"steps"}, {"phase":"eval"}
            ]),
            false,
        )
        .unwrap();
        let quote = |amount_micros, per| {
            Ok(Some(Quote {
                amount_micros,
                per,
                currency: "USD".into(),
                source: "huggingface.co".into(),
                quoted_at: 0,
            }))
        };
        let h200 = quote(6_000_000, Per::Hour);
        let text = report(
            &plan[..2],
            Some(&parent()),
            "--backend hf --flavor h200",
            &h200,
        );
        assert!(
            text.contains("Price: $6.00/hr for --backend hf --flavor h200, from huggingface.co"),
            "{text}"
        );
        assert!(text.contains("Cost: about $3.10"), "{text}");
        let text = report(&plan, Some(&parent()), "--backend hf --flavor h200", &h200);
        assert!(text.contains("Cost: at least $3.10"), "{text}");
        assert!(text.lines().last().unwrap().contains("duration or cost"));

        let cpu = quote(167, Per::Minute);
        let text = report(
            &plan[..2],
            Some(&parent()),
            "--backend hf --flavor cpu-basic",
            &cpu,
        );
        assert!(text.contains("Price: $0.01/hr"), "{text}");
        assert!(text.contains("Cost: about $0.0052"), "{text}");

        let text = report(&plan[..2], Some(&parent()), "--backend ssh", &Ok(None));
        assert!(
            text.contains("Cost: orx has no price for --backend ssh"),
            "{text}"
        );
        let missing = Err(anyhow!("pass the --flavor the run will use"));
        let text = report(&plan[..2], Some(&parent()), "--backend hf", &missing);
        assert!(
            text.contains("Cost: unknown for --backend hf: pass the --flavor"),
            "{text}"
        );
    }

    #[test]
    fn a_plan_no_run_has_timed_gives_the_hourly_price_and_no_total() {
        let plan = orx_signals::plan(&json!([{"phase":"train", "of":600, "unit":"steps"}]), false)
            .unwrap();
        let h200 = Ok(Some(Quote {
            amount_micros: 6_000_000,
            per: Per::Hour,
            currency: "USD".into(),
            source: "huggingface.co".into(),
            quoted_at: 0,
        }));
        let text = report(&plan, None, "--backend hf --flavor h200", &h200);
        assert!(text.contains("Total: unknown (train not timed)"), "{text}");
        assert!(text.contains("Price: $6.00/hr"), "{text}");
        assert!(text.contains("Cost: unknown"), "{text}");
        assert!(!text.contains("at least"), "{text}");
    }

    #[test]
    fn sizing_reports_subsecond_parent_steps() {
        let mut snapshot = orx_signals::Snapshot::new("parent", "log", "generation");
        snapshot
            .feed(
                b"::orx:: {\"phase\":\"train\",\"step\":0,\"of\":60,\"unit\":\"steps\"}\n",
                false,
                0,
                true,
            )
            .unwrap();
        snapshot
            .feed(b"::orx:: {\"step\":60}\n", true, 6_000, true)
            .unwrap();
        let plan =
            orx_signals::plan(&json!([{"phase":"train", "of":60, "unit":"steps"}]), false).unwrap();
        let text = report(&plan, Some(&snapshot), "--backend local", &Ok(None));
        assert!(
            text.contains("train: about 6 s — 60 steps; the parent took 100 ms each"),
            "{text}"
        );
        assert!(text.contains("Total: about 6 s"), "{text}");
    }

    #[test]
    fn sizing_matches_the_live_prior_when_the_parent_finishes_below_its_total() {
        let mut parent = orx_signals::Snapshot::new("parent", "log", "generation");
        parent
            .feed(
                b"::orx:: {\"phase\":\"train\",\"step\":0,\"of\":100,\"unit\":\"steps\"}\n",
                false,
                0,
                true,
            )
            .unwrap();
        parent
            .feed(b"::orx:: {\"step\":80}\n", true, 100_000, true)
            .unwrap();
        let plan = orx_signals::plan(&json!([{"phase":"train", "of":100, "unit":"steps"}]), false)
            .unwrap();
        let stages = estimate::stages(
            &plan,
            None,
            false,
            &[(&parent, true, estimate::TimedBy::Parent)],
        );
        assert_eq!(stages[0].prior.unwrap().ms, 100_000.0);

        let text = report(&plan, Some(&parent), "--backend local", &Ok(None));
        assert!(text.contains("Total: about 1 min 40 s"), "{text}");
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
        assert!(report(&plan, Some(&recorded), "--backend local", &Ok(None))
            .contains("train not timed"));
        recorded.phases[1].done = None;
        let plan = orx_signals::plan(
            &json!([
                {"phase":"train", "of":600}
            ]),
            false,
        )
        .unwrap();
        assert!(report(&plan, Some(&recorded), "--backend local", &Ok(None))
            .contains("train not timed"));
        assert!(report(&plan, Some(&parent()), "--backend local", &Ok(None))
            .contains("Total: about 30 min"));
    }
}
