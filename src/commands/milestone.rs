//! `orx milestone`: the chat's agent tells the status report orx keeps for the user what changed.

use reqwest::StatusCode;

use crate::error::{anyhow, Context, Result};

pub async fn run(args: crate::MilestoneArgs) -> Result<()> {
    println!("{}", noted(&args.note).await?);
    Ok(())
}

#[derive(serde::Deserialize)]
struct Answer {
    noted: bool,
    why: Option<String>,
}

/// What the agent is told once this chat's `orx up` took `note` or said why it did not.
pub(crate) async fn noted(note: &str) -> Result<String> {
    let chat = crate::local::chat::launching_chat_session().ok_or_else(|| {
        anyhow!("`orx milestone` notes a milestone in the status report of the chat it runs in; run it from the agent's shell in an OpenResearch chat.")
    })?;
    let path = format!("chats/{}/milestone", urlencoding::encode(&chat));
    let response = crate::plane::local_plane::ApiClient::discover()
        .await?
        .request(reqwest::Method::POST, &path)
        .json(&serde_json::json!({ "note": note }))
        .send()
        .await
        .context("Couldn't reach this chat's `orx up` to note the milestone; retry `orx milestone` once it is running")?;
    let status = response.status();
    let body = response.text().await.with_context(|| {
        format!(
            "Couldn't read what this chat's `orx up` answered ({status}); retry `orx milestone`"
        )
    })?;
    if status == StatusCode::NOT_FOUND {
        let version = std::env::var(crate::local::chat::UP_VERSION_ENV)
            .map_or_else(|_| String::new(), |version| format!(" ({version})"));
        return Ok(format!(
            "This chat's `orx up`{version} predates status reports, so nothing was noted; carry on without it."
        ));
    }
    if status == StatusCode::SERVICE_UNAVAILABLE {
        return Err(anyhow!(
            "This chat's `orx up` is busy, so nothing was noted yet; retry `orx milestone` in a moment."
        ));
    }
    if !status.is_success() {
        return Err(anyhow!("{body}"));
    }
    let answer: Answer = serde_json::from_str(&body).map_err(|_| {
        anyhow!("This chat's `orx up` answered `{body}`, which this orx can't read, so the milestone may not have been noted.")
    })?;
    Ok(match answer {
        Answer { noted: true, .. } => {
            "Noted. The status report updates in the background; carry on.".into()
        }
        Answer { why, .. } => format!(
            "{} Nothing was noted; carry on.",
            why.unwrap_or_else(|| "This chat keeps no status report.".into())
        ),
    })
}
