//! `orx instructions`: print the user's OpenResearch instructions, or propose a
//! new version on a card in the chat the agent runs in.

use std::path::Path;

use serde_json::{json, Value};

use crate::error::{anyhow, bail, Result};
use crate::local::chat::instructions;
use crate::InstructionsCommand;

pub async fn run(args: crate::InstructionsArgs) -> Result<()> {
    match args.command {
        None | Some(InstructionsCommand::Show) => {
            let snapshot = instructions::read(&instructions::path())?;
            if snapshot.content.trim().is_empty() {
                eprintln!("No OpenResearch instructions yet.");
            } else {
                print!("{}", snapshot.content);
            }
            Ok(())
        }
        Some(InstructionsCommand::Propose { path }) => {
            let told = propose(&path, |name| std::env::var(name).ok()).await?;
            println!("{told}");
            Ok(())
        }
    }
}

async fn propose(draft: &Path, var: impl Fn(&str) -> Option<String>) -> Result<String> {
    let content = std::fs::read_to_string(draft).map_err(|error| {
        anyhow!(
            "Couldn't read the draft at {}: {error}. Write the whole new instructions to a file and pass its path.",
            draft.display()
        )
    })?;
    let (Some(port), Some(session), Some(token)) = (
        var("ORX_UP_PORT").filter(|value| !value.is_empty()),
        var("ORX_SESSION_ID").filter(|value| !value.is_empty()),
        var("ORX_AGENT_TOKEN").filter(|value| !value.is_empty()),
    ) else {
        bail!(
            "`orx instructions propose` runs inside an OpenResearch chat, where it asks the user to approve the change. \
             Outside one, edit {} directly or in OpenResearch under Customize.",
            instructions::path().display()
        );
    };
    let url = format!("http://127.0.0.1:{port}/api/internal/mcp");
    let response = crate::net::loopback_client()
        .connect_timeout(std::time::Duration::from_secs(3))
        .build()?
        .post(&url)
        .json(&json!({
            "sessionId": session,
            "token": token,
            "method": "tools/call",
            "params": { "name": "propose_instructions", "arguments": { "content": content } },
        }))
        .send()
        .await
        .map_err(|error| {
            anyhow!("Couldn't reach the OpenResearch app running this chat ({error}). Make sure OpenResearch is open, then run this again.")
        })?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    let text = body
        .pointer("/content/0/text")
        .or_else(|| body.get("error"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("OpenResearch answered with HTTP {status}."));
    if !status.is_success() || body["isError"] == true {
        bail!("{text}");
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn propose_names_what_is_missing() {
        let missing = crate::config::test_home().join("no-such-draft.md");
        let error = propose(&missing, |_| Some("1".into()))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("Couldn't read the draft"), "{error}");
        assert!(error.contains("no-such-draft.md"), "{error}");

        let draft =
            crate::config::test_home().join(format!("draft-{}.md", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(draft.parent().unwrap()).unwrap();
        std::fs::write(&draft, "Name runs after the idea.").unwrap();
        let error = propose(&draft, |_| None).await.unwrap_err().to_string();
        assert!(error.contains("inside an OpenResearch chat"), "{error}");

        // Nothing listens on port 1.
        let error = propose(&draft, |name| {
            Some(if name == "ORX_UP_PORT" { "1" } else { "x" }.into())
        })
        .await
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("Couldn't reach the OpenResearch app"),
            "{error}"
        );
    }
}
