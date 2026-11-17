use crate::error::{anyhow, Result};
use std::process::Stdio;
use std::time::Duration;

pub fn check() -> Result<()> {
    gstreamer::init()?;
    for name in [
        "appsrc",
        "appsink",
        "playbin",
        "decodebin",
        "autoaudiosink",
        "audioconvert",
        "audioresample",
        "volume",
        "scaletempo",
        "qtdemux",
        "videoconvert",
    ] {
        gstreamer::ElementFactory::make(name)
            .build()
            .map_err(|error| anyhow!("Cannot create GStreamer element {name}: {error}"))?;
    }
    Ok(())
}

pub async fn available() -> bool {
    let result = async {
        // Preserve the selected runtime, unlike commands launched for the user's tools.
        let mut command = crate::process::tokio_command(std::env::current_exe()?);
        match std::env::var_os("LD_LIBRARY_PATH") {
            Some(path) => command.env("LD_LIBRARY_PATH", path),
            None => command.env_remove("LD_LIBRARY_PATH"),
        };
        let mut child = command
            .arg("check-media")
            // Plugin scanning belongs to this disposable child, not an orphanable helper.
            .env("GST_REGISTRY_FORK", "no")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        match tokio::time::timeout(Duration::from_secs(3), child.wait()).await {
            Ok(status) => {
                let status = status?;
                if !status.success() {
                    return Err(anyhow!("Media check exited with {status}"));
                }
                Ok(())
            }
            Err(_) => {
                child.kill().await?;
                Err(anyhow!("Media check timed out"))
            }
        }
    }
    .await;
    if let Err(error) = result {
        eprintln!("openresearch app: audio/video disabled: {error:#}");
        false
    } else {
        true
    }
}
