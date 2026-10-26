use crate::error::Result;

pub async fn run(args: crate::RefreshArgs) -> Result<()> {
    let run = match crate::local::chat::trusted_up_port()? {
        Some(port) => {
            crate::commands::up::require_ssh_monitoring_support(port).await?;
            crate::commands::up::chat_request_via_up(
                port,
                reqwest::Method::POST,
                &format!("/api/runs/{}/refresh", args.run_id),
                &serde_json::json!({"collectLogs":args.collect_logs}),
            )
            .await?
        }
        None => crate::commands::up::run_response(
            &crate::jobs::ssh::monitor::refresh(&args.run_id, args.collect_logs).await?,
        ),
    };
    if args.json {
        println!("{}", serde_json::to_string_pretty(&run)?);
    } else {
        let r = run.get("run").unwrap_or(&run);
        println!(
            "Run {}: {}",
            args.run_id,
            r["status"].as_str().unwrap_or("unknown")
        );
        println!("{}", serde_json::to_string_pretty(&run)?);
    }
    Ok(())
}
