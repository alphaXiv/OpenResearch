use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

use super::super::SshTarget;
use crate::error::{anyhow, Result};

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Resolved {
    pub key: String,
    pub target: SshTarget,
    pub options: Vec<String>,
    pub socket: Option<PathBuf>,
}

pub(super) fn root() -> PathBuf {
    let namespace = Sha256::digest(crate::config::config_dir().to_string_lossy().as_bytes());
    crate::store::data_dir().join(format!("ssh-service-{:x}", namespace))
}

pub(super) fn database() -> Result<Connection> {
    open_database(&root().join("state.sqlite3"))
}

fn open_database(path: &std::path::Path) -> Result<Connection> {
    let db = Connection::open(path)?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    db.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
        CREATE TABLE IF NOT EXISTS connections (
            id TEXT PRIMARY KEY, resolved TEXT NOT NULL, failures INTEGER NOT NULL DEFAULT 0,
            next_at INTEGER NOT NULL DEFAULT 0, last_error TEXT);
        CREATE TABLE IF NOT EXISTS bindings (run TEXT PRIMARY KEY, connection TEXT NOT NULL);",
    )?;
    Ok(db)
}

pub(super) async fn resolve(target: &SshTarget) -> Result<Resolved> {
    if let Some(run) = &target.run_id {
        let saved: Option<String> = database()?.query_row(
            "SELECT resolved FROM connections JOIN bindings ON connections.id = bindings.connection WHERE run = ?1",
            [run], |row| row.get(0)).optional()?;
        if let Some(saved) = saved {
            return Ok(serde_json::from_str(&saved)?);
        }
    }
    super::super::prepare_control_dir()?;
    #[cfg(unix)]
    if target.extra_opts.is_empty() {
        super::super::prepared::prepare(target, false).await?;
    }
    let options = super::super::ssh_opts(target, true);
    let key = configuration_key(target, &options).await?;
    #[cfg(unix)]
    let socket = Some(super::super::resolved_control_path(target).await?);
    #[cfg(not(unix))]
    let socket = None;
    let resolved = Resolved {
        key,
        target: target.clone(),
        options,
        socket,
    };
    let mut db = database()?;
    let tx = db.transaction()?;
    tx.execute(
        "INSERT OR IGNORE INTO connections (id, resolved) VALUES (?1, ?2)",
        params![resolved.key, serde_json::to_string(&resolved)?],
    )?;
    if let Some(run) = &target.run_id {
        tx.execute(
            "INSERT OR IGNORE INTO bindings (run, connection) VALUES (?1, ?2)",
            params![run, resolved.key],
        )?;
    }
    let resolved = if let Some(run) = &target.run_id {
        let saved: String = tx.query_row("SELECT resolved FROM connections JOIN bindings ON connections.id = bindings.connection WHERE run = ?1", [run], |row| row.get(0))?;
        serde_json::from_str(&saved)?
    } else {
        resolved
    };
    tx.commit()?;
    Ok(resolved)
}

pub(super) async fn validate_configuration(resolved: &Resolved) -> Result<()> {
    if configuration_key(&resolved.target, &resolved.options).await? != resolved.key {
        return Err(anyhow!("SSH configuration changed for this run's pinned connection. Restore its original route/authentication settings or start a new run with the new connection."));
    }
    Ok(())
}

async fn configuration_key(target: &SshTarget, options: &[String]) -> Result<String> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::process::Command::new("ssh")
            .env("ORX_SSH_PROBE", "1")
            .arg("-G")
            .args(options)
            .args(["--", &target.dest])
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    if !output.status.success() {
        return Err(anyhow!(
            "Could not resolve SSH route: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let configuration = String::from_utf8(output.stdout)?;
    let mut digest = Sha256::new();
    // Preserve the complete resolved auth/proxy context; never group by hostname alone.
    for line in configuration
        .lines()
        .filter(|line| !line.starts_with("host "))
    {
        digest.update(line.as_bytes());
        digest.update(b"\n");
    }
    digest.update(serde_json::to_vec(&target.managed_generation)?);
    digest.update(serde_json::to_vec(&target.auth_agent)?);
    Ok(format!("{:x}", digest.finalize()))
}

pub(super) fn retry_at(key: &str) -> Result<i64> {
    Ok(database()?.query_row(
        "SELECT next_at FROM connections WHERE id = ?1",
        [key],
        |row| row.get(0),
    )?)
}

pub(super) fn reserve_attempt(key: &str) -> Result<i64> {
    reserve_in(&mut database()?, key)
}

fn reserve_in(db: &mut Connection, key: &str) -> Result<i64> {
    let tx = db.transaction()?;
    let failures: u32 = tx.query_row(
        "SELECT failures FROM connections WHERE id = ?1",
        [key],
        |row| row.get(0),
    )?;
    let nominal = delay_seconds(failures);
    let random = u16::from_le_bytes(uuid::Uuid::new_v4().as_bytes()[..2].try_into()?);
    let jitter = 0.8 + f64::from(random) / f64::from(u16::MAX) * 0.4;
    let delay = ((nominal as f64 * jitter).min(300.0) * 1000.0) as i64;
    let next = crate::store::now_ms() + delay;
    tx.execute(
        "UPDATE connections SET failures = MIN(failures + 1, 31), next_at = ?2 WHERE id = ?1",
        params![key, next],
    )?;
    tx.commit()?;
    Ok(next)
}

fn delay_seconds(failures: u32) -> u64 {
    (5u64 << failures.min(6)).min(300)
}

pub(super) fn healthy(key: &str) -> Result<()> {
    database()?.execute(
        "UPDATE connections SET failures = 0, next_at = 0, last_error = NULL WHERE id = ?1",
        [key],
    )?;
    Ok(())
}

pub(super) fn failure(key: &str, error: &str) -> Result<()> {
    let db = database()?;
    let failures: u32 = db.query_row(
        "SELECT failures FROM connections WHERE id = ?1",
        [key],
        |row| row.get(0),
    )?;
    let next = crate::store::now_ms() + delay_seconds(failures.saturating_sub(1)) as i64 * 1000;
    db.execute("UPDATE connections SET last_error = ?2, next_at = MAX(next_at, ?3), failures = MAX(failures, 1) WHERE id = ?1", params![key, error, next])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[tokio::test]
    async fn pinned_connection_rejects_changed_identity_before_authentication() {
        let temp = crate::local::git::TemporaryDirectory::new("orx-route-pin").unwrap();
        let path = temp.path().join("config");
        std::fs::write(
            &path,
            "Host lab\n HostName 127.0.0.1\n IdentityFile /original-key\n",
        )
        .unwrap();
        let target = crate::jobs::ssh::SshTarget::alias("lab");
        let options = vec!["-F".into(), path.to_string_lossy().into_owned()];
        let resolved = super::Resolved {
            key: super::configuration_key(&target, &options).await.unwrap(),
            target,
            options,
            socket: None,
        };
        super::validate_configuration(&resolved).await.unwrap();
        std::fs::write(
            &path,
            "Host lab\n HostName 127.0.0.1\n IdentityFile /replacement-key\n",
        )
        .unwrap();
        assert!(super::validate_configuration(&resolved)
            .await
            .unwrap_err()
            .to_string()
            .contains("configuration changed"));
    }

    #[test]
    fn retry_history_survives_restart_and_new_run() {
        let temp = crate::local::git::TemporaryDirectory::new("orx-backoff-test").unwrap();
        let path = temp.path().join("state.db");
        let mut db = super::open_database(&path).unwrap();
        db.execute(
            "INSERT INTO connections(id,resolved) VALUES ('host','{}')",
            [],
        )
        .unwrap();
        let first = super::reserve_in(&mut db, "host").unwrap();
        drop(db);
        let mut db = super::open_database(&path).unwrap();
        db.execute(
            "INSERT OR IGNORE INTO connections(id,resolved) VALUES ('host','{}')",
            [],
        )
        .unwrap();
        db.execute("INSERT INTO bindings VALUES ('new-run','host')", [])
            .unwrap();
        let persisted: (u32, i64) = db
            .query_row(
                "SELECT failures,next_at FROM connections WHERE id='host'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(persisted, (1, first));
        let before = crate::store::now_ms();
        let second = super::reserve_in(&mut db, "host").unwrap();
        assert!((8_000..=12_100).contains(&(second - before)));
    }

    #[test]
    fn backoff_caps_without_overflow() {
        assert_eq!(
            (0..8).map(super::delay_seconds).collect::<Vec<_>>(),
            [5, 10, 20, 40, 80, 160, 300, 300]
        );
        assert_eq!(super::delay_seconds(u32::MAX), 300);
    }
}
