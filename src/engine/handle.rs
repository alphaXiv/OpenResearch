use serde::{Deserialize, Serialize};

use crate::error::{anyhow, Result};
use crate::jobs::BackendDescriptor;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ApiAccount {
    pub target: Option<String>,
    pub context: Option<String>,
}

impl ApiAccount {
    pub fn target(&self) -> Result<&str> {
        self.target
            .as_deref()
            .ok_or_else(|| anyhow!("API account has no target"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Handle {
    Ssh {
        host: String,
        root: String,
    },
    Slurm {
        host: String,
        root: String,
        job: Option<String>,
    },
    Local {
        dir: String,
    },
    Tinker {
        dir: String,
    },
    Api {
        provider: String,
        account: ApiAccount,
        ids: Vec<String>,
    },
    OpenResearch {
        descriptor: Box<BackendDescriptor>,
    },
    Legacy {
        descriptor: Box<BackendDescriptor>,
    },
}

pub fn backend_json(
    run_id: &str,
    handle: Option<&Handle>,
    metadata: &BackendDescriptor,
) -> Result<String> {
    let metadata = match handle {
        Some(Handle::Legacy { descriptor } | Handle::OpenResearch { descriptor }) => {
            descriptor.as_ref()
        }
        _ => metadata,
    };

    let job_id = match handle {
        None => None,
        Some(
            Handle::Ssh { root, .. }
            | Handle::Slurm {
                root, job: None, ..
            },
        ) => Some(format!("{}/runs/{run_id}", root.trim_end_matches('/'))),
        Some(Handle::Slurm { job: Some(job), .. }) => Some(job.clone()),
        Some(Handle::Local { dir } | Handle::Tinker { dir }) => Some(dir.clone()),
        Some(Handle::Api { ids, .. }) => ids.first().cloned(),
        Some(Handle::Legacy { descriptor } | Handle::OpenResearch { descriptor }) => {
            descriptor.job_id.clone()
        }
    };

    if handle.is_some() && job_id.as_deref().is_none_or(str::is_empty) {
        return Err(anyhow!("Engine handle has no job id"));
    }

    let mut value = serde_json::json!({ "kind": "engine" });
    if let Some(job_id) = job_id {
        value["jobId"] = job_id.into();
    }
    if let Some(digest) = &metadata.source_digest {
        value["sourceDigest"] = digest.clone().into();
    }
    if let Some(path) = &metadata.source_path {
        value["sourcePath"] = path.clone().into();
    }
    if let Some(size) = metadata.source_size {
        value["sourceSize"] = size.into();
    }
    if let Some(handle) = handle {
        value["handle"] = serde_json::to_value(handle)?;
    }

    Ok(serde_json::to_string(&value)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_handle_keeps_old_descriptor_fields_and_round_trips() {
        let metadata = BackendDescriptor::parse(
            r#"{"kind":"ssh_job","jobId":"stale","sourceDigest":"abc","sourcePath":"/sources/abc.tar","sourceSize":123}"#,
        ).unwrap();
        let handles = [
            (
                Handle::Ssh {
                    host: "host".into(),
                    root: "/orx/".into(),
                },
                "/orx/runs/run",
            ),
            (
                Handle::Slurm {
                    host: "login".into(),
                    root: "/shared".into(),
                    job: Some("1234".into()),
                },
                "1234",
            ),
            (
                Handle::Slurm {
                    host: "login".into(),
                    root: "/shared".into(),
                    job: None,
                },
                "/shared/runs/run",
            ),
            (
                Handle::Local {
                    dir: "/local/run".into(),
                },
                "/local/run",
            ),
            (
                Handle::Tinker {
                    dir: "/local/tinker".into(),
                },
                "/local/tinker",
            ),
            (
                crate::engine::adapters::api::test_handle(
                    "hf_job",
                    Some("lab"),
                    None,
                    vec!["hf-1".into()],
                ),
                "hf-1",
            ),
            (
                crate::engine::adapters::api::test_handle(
                    "modal_job",
                    None,
                    None,
                    vec!["sb-1".into()],
                ),
                "sb-1",
            ),
            (
                crate::engine::adapters::api::test_handle(
                    "k8s_job",
                    Some("lab"),
                    Some("cluster"),
                    vec!["job/run".into(), "configmap/run".into()],
                ),
                "job/run",
            ),
            (
                crate::engine::adapters::api::test_handle(
                    "ray_job",
                    Some("http://ray"),
                    None,
                    vec!["ray-1".into()],
                ),
                "ray-1",
            ),
            (
                Handle::Legacy {
                    descriptor: Box::new(metadata.clone()),
                },
                "stale",
            ),
        ];
        let pending = backend_json("run", None, &metadata).unwrap();
        let pending_old = BackendDescriptor::parse(&pending).unwrap();
        assert_eq!(pending_old.kind, "engine");
        assert!(pending_old.job_id.is_none());
        assert_eq!(pending_old.source_digest, metadata.source_digest);
        for (handle, id) in handles {
            let json = backend_json("run", Some(&handle), &metadata).unwrap();
            let old = BackendDescriptor::parse(&json).unwrap();
            assert_eq!(old.kind, "engine");
            assert_eq!(old.job_id.as_deref(), Some(id));
            assert_eq!(old.source_digest, metadata.source_digest);
            assert_eq!(old.source_path, metadata.source_path);
            assert_eq!(old.source_size, metadata.source_size);
            let value: serde_json::Value = serde_json::from_str(&json).unwrap();
            let decoded: Handle = serde_json::from_value(value["handle"].clone()).unwrap();
            assert_eq!(
                serde_json::to_value(&decoded).unwrap(),
                serde_json::to_value(&handle).unwrap()
            );
            assert_eq!(
                serde_json::to_value(old).unwrap()["handle"],
                value["handle"]
            );
        }
    }

    /// Stored rows outlive the version that wrote them, so every engine handle
    /// in the contract fixture decodes and re-encodes unchanged.
    #[test]
    fn stored_engine_handles_decode() {
        let stored: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../../tests/contracts/run-descriptors.json"))
                .unwrap();
        for row in stored.iter().filter(|row| row["kind"] == "engine") {
            let handle: Handle = serde_json::from_value(row["handle"].clone()).unwrap();
            assert_eq!(serde_json::to_value(&handle).unwrap(), row["handle"]);
        }
    }

    #[test]
    fn legacy_handle_preserves_one_rental_and_endpoint_and_public_json_hides_token() {
        let descriptor = BackendDescriptor::parse(
            r#"{"kind":"openresearch_job","namespace":"org","jobId":"sb","sshHost":"compute","sshPort":2200,"sshUser":"root","rentalLease":{"apiUrl":"https://example.invalid","sandboxId":"sb","token":"private-token"},"sourceDigest":"snapshot","sourcePath":"/snapshot.tar","sourceSize":456}"#,
        ).unwrap();
        let json = backend_json(
            "run",
            Some(&Handle::Legacy {
                descriptor: Box::new(descriptor),
            }),
            &BackendDescriptor::parse(r#"{"kind":"local_job"}"#).unwrap(),
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(json.matches("private-token").count(), 1);
        assert!(value.get("rentalLease").is_none());
        assert!(value.get("namespace").is_none());
        assert!(value.get("sshHost").is_none());
        let inner: BackendDescriptor =
            serde_json::from_value(value["handle"]["descriptor"].clone()).unwrap();
        assert_eq!(inner.openresearch_ref().unwrap(), ("org", "sb"));
        assert_eq!(
            inner.openresearch_ssh_target().unwrap().dest,
            "root@compute"
        );
        assert_eq!(inner.rental_lease.unwrap().token, "private-token");
        let old = BackendDescriptor::parse(&json).unwrap();
        assert_eq!(old.source_digest.as_deref(), Some("snapshot"));
        assert_eq!(old.source_path.as_deref(), Some("/snapshot.tar"));
        assert_eq!(old.source_size, Some(456));
        let public = crate::store::public_backend(&json, None).unwrap();
        assert_eq!(public["kind"], "openresearch_job");
        assert!(!public.to_string().contains("private-token"));
        assert!(public.get("rentalLease").is_none());
        assert!(public["handle"]["descriptor"].get("rentalLease").is_none());
        assert_eq!(public["handle"]["descriptor"]["jobId"], "sb");
    }

    #[test]
    fn handles_without_a_legacy_visible_id_are_rejected() {
        let metadata = BackendDescriptor::parse(r#"{"kind":"ssh_job"}"#).unwrap();
        for handle in [
            crate::engine::adapters::api::test_handle("k8s_job", Some("lab"), None, vec![]),
            Handle::Legacy {
                descriptor: Box::new(metadata.clone()),
            },
            crate::engine::adapters::api::test_handle(
                "hf_job",
                Some("lab"),
                None,
                vec![String::new()],
            ),
        ] {
            assert!(backend_json("run", Some(&handle), &metadata).is_err());
        }
    }
}
