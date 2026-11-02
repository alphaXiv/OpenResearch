//! `orx metrics <runId>…`: what runs logged to Trackio, summarized for reading curves.

use std::fmt::Write as _;

use crate::error::{anyhow, Result};
use crate::local::trackio::{self, MetricSummary, Point, RunMetrics};
use crate::store::Store;

pub async fn run(args: crate::MetricsArgs) -> Result<()> {
    let store = Store::open()?;
    // Dropped at exit, which stops a local dashboard this command had to start.
    let local = std::sync::Arc::new(trackio::LocalServer::default());
    let mut reports = Vec::new();
    for id in &args.run_ids {
        let run = crate::local::resolve::resolve_run(&store, id)?;
        let project = store
            .get_local_project(&run.project_id)?
            .ok_or_else(|| anyhow!("Run {}'s project is no longer in the local store.", run.id))?;
        let report = trackio::run_metrics(
            &local,
            trackio::project_for(&project.slug),
            run.id,
            &args.metrics,
            args.points,
        )
        .await;
        reports.push(report);
    }
    if args.json {
        println!("{}", serde_json::to_string_pretty(&reports)?);
    } else {
        let text: Vec<String> = reports.iter().map(render).collect();
        print!("{}", text.join("\n"));
    }
    if reports
        .iter()
        .all(|report| report.logged_steps.is_none() && !report.errors.is_empty())
    {
        return Err(anyhow!("Could not read Trackio for any of these runs."));
    }
    Ok(())
}

fn render(report: &RunMetrics) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "Run {} · Trackio project {}",
        report.run, report.project
    );
    if let Some(group) = &report.group {
        let _ = write!(out, " · group {group}");
    }
    out.push('\n');
    for error in &report.errors {
        let _ = writeln!(out, "Could not read: {error}");
    }
    let Some(steps) = report.logged_steps else {
        if report.dashboard_url.is_none() && report.errors.is_empty() {
            let _ = match trackio::target() {
                trackio::Target::Space(id) => writeln!(
                    out,
                    "Space {id} does not exist yet, or HF_TOKEN cannot see it; Trackio creates it on the first logged run."
                ),
                _ => writeln!(
                    out,
                    "Nothing logged to this machine's Trackio store for this project, and no Space or server is saved. Remote runs log to Trackio only with {} or {} saved.",
                    trackio::SPACE_ID,
                    trackio::SERVER_URL
                ),
            };
        } else if report.errors.is_empty() {
            let _ = writeln!(
                out,
                "Nothing logged to Trackio for this run. It logs only if the code calls trackio.init with name=ORX_RUN_ID."
            );
        }
        return out;
    };
    let last = report
        .last_step
        .map(|step| format!(", last step {step}"))
        .unwrap_or_default();
    let _ = writeln!(out, "Logged {steps} steps{last}");
    if let Some(config) = report.config.as_ref().filter(|config| !config.is_empty()) {
        let fields: Vec<String> = config
            .iter()
            .map(|(key, value)| match value.as_str() {
                Some(text) => format!("{key}={text}"),
                None => format!("{key}={value}"),
            })
            .collect();
        let _ = writeln!(out, "Config: {}", fields.join(", "));
    }
    for alert in &report.alerts {
        let step = alert["step"]
            .as_i64()
            .map(|step| format!(" (step {step})"))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "Alert [{}] {}: {}{step}",
            alert["level"].as_str().unwrap_or("info"),
            alert["title"].as_str().unwrap_or_default(),
            alert["text"].as_str().unwrap_or_default(),
        );
    }
    for metric in &report.metrics {
        out.push('\n');
        out.push_str(&render_metric(metric));
    }
    if !report.other_metrics.is_empty() {
        let _ = writeln!(out, "\nNon-numeric: {}", report.other_metrics.join(", "));
    }
    out
}

fn render_metric(metric: &MetricSummary) -> String {
    let at = |point: &Point| format!("{}@{}", value(point.value), point.step);
    let series: Vec<String> = metric
        .series
        .iter()
        .map(|point| format!("{}:{}", point.step, value(point.value)))
        .collect();
    format!(
        "{}  {} pts  first {} → last {}  min {}  max {}  last-10% mean {}\n  {}\n  {}\n",
        metric.name,
        metric.points,
        at(&metric.first),
        at(&metric.last),
        at(&metric.min),
        at(&metric.max),
        value(metric.tail_mean),
        sparkline(&metric.series),
        series.join(" "),
    )
}

/// Four significant figures, switching to exponent form outside a readable range.
fn value(v: f64) -> String {
    let magnitude = v.abs();
    if v == 0.0 {
        "0".to_string()
    } else if (1e-3..1e5).contains(&magnitude) {
        let decimals = (3 - magnitude.log10().floor() as i32).clamp(0, 6) as usize;
        let text = format!("{v:.decimals$}");
        if text.contains('.') {
            text.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            text
        }
    } else {
        format!("{v:.3e}")
    }
}

fn sparkline(series: &[Point]) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let lo = series.iter().map(|p| p.value).fold(f64::INFINITY, f64::min);
    let hi = series
        .iter()
        .map(|p| p.value)
        .fold(f64::NEG_INFINITY, f64::max);
    series
        .iter()
        .map(|point| {
            if hi > lo {
                BARS[(((point.value - lo) / (hi - lo)) * 7.0).round() as usize]
            } else {
                BARS[3]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_read_as_four_significant_figures() {
        assert_eq!(value(0.054_881), "0.05488");
        assert_eq!(value(1.0), "1");
        assert_eq!(value(1234.56), "1235");
        assert_eq!(value(0.0), "0");
        assert_eq!(value(3.2e-6), "3.200e-6");
        assert_eq!(value(-2.5), "-2.5");
    }

    #[test]
    fn sparkline_spans_the_series_range() {
        let points: Vec<Point> = [3.0, 2.0, 1.0, 1.0]
            .iter()
            .enumerate()
            .map(|(step, &value)| Point {
                step: step as i64,
                value,
            })
            .collect();
        assert_eq!(sparkline(&points), "█▅▁▁");
        assert_eq!(sparkline(&points[2..]), "▄▄");
    }
}
