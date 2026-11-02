use serde::{de::DeserializeOwned, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::error::{anyhow, Result};

pub(super) const LIMIT: usize = 1024 * 1024;

pub(super) async fn read<T: DeserializeOwned>(pipe: &mut (impl AsyncRead + Unpin)) -> Result<T> {
    let size = pipe.read_u32().await? as usize;
    if size > LIMIT {
        return Err(anyhow!("SSH service frame exceeds limit"));
    }
    let mut bytes = vec![0; size];
    pipe.read_exact(&mut bytes).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub(super) async fn write<T: Serialize>(
    pipe: &mut (impl AsyncWrite + Unpin),
    value: &T,
) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > LIMIT {
        return Err(anyhow!("SSH service frame exceeds limit"));
    }
    pipe.write_u32(bytes.len() as u32).await?;
    pipe.write_all(&bytes).await?;
    pipe.flush().await?;
    Ok(())
}
