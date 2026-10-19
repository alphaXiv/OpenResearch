//! Explicit imports bind measured values to an existing run, never to a guessed latest run.
use std::io::Read;

use crate::error::{anyhow, Result};
use crate::store::{
    run_metrics::{RunMetrics, MAX_BYTES},
    Store,
};

#[derive(Debug, clap::Args)]
pub struct MetricsArgs {
    pub exp_id: String,
    #[arg(long)]
    pub run: String,
    /// Version-1 metrics JSON file; - reads stdin. Omit to inspect saved metrics.
    #[arg(long)]
    pub file: Option<String>,
    /// Explicitly overwrite measurements already imported for this run.
    #[arg(long, requires = "file")]
    pub replace: bool,
}

pub fn run(store: &Store, args: MetricsArgs) -> Result<()> {
    let run = store
        .get_run(&args.run)?
        .ok_or_else(|| anyhow!("Run not found."))?;
    if run.experiment_id != args.exp_id {
        return Err(anyhow!("The run does not belong to this experiment."));
    }
    if let Some(path) = args.file {
        let input: Box<dyn Read> = if path == "-" {
            Box::new(std::io::stdin())
        } else {
            Box::new(std::fs::File::open(path)?)
        };
        let mut bytes = Vec::new();
        input.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > MAX_BYTES {
            return Err(anyhow!("Metrics exceed the 64 KiB limit."));
        }
        let data: RunMetrics = serde_json::from_slice(&bytes)?;
        store.put_run_metrics(&args.run, &data, args.replace)?;
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&store.get_run_metrics(&args.run)?)?
    );
    Ok(())
}
