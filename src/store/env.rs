use super::*;

/// A variable saved for one project. For that project's agents and runs it
/// overrides the same key in `~/.openresearch/env`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectEnvVar {
    pub key: String,
    pub value: String,
    pub description: Option<String>,
}

impl Store {
    pub fn project_env(&self, project_id: &str) -> Result<Vec<ProjectEnvVar>> {
        let mut statement = self.conn.prepare(
            "SELECT key, value, description FROM project_env_vars
             WHERE project_id = ?1 ORDER BY created_at, key",
        )?;
        let rows = statement.query_map([project_id], |row| {
            Ok(ProjectEnvVar {
                key: row.get(0)?,
                value: row.get(1)?,
                description: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Saves `value`, keeping the key's description.
    pub fn set_project_env(&self, project_id: &str, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO project_env_vars (project_id, key, value, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (project_id, key) DO UPDATE SET value = excluded.value",
            params![project_id, key, value, now_ms()],
        )?;
        Ok(())
    }

    pub fn delete_project_env(&self, project_id: &str, key: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM project_env_vars WHERE project_id = ?1 AND key = ?2",
            params![project_id, key],
        )?;
        Ok(())
    }

    /// Descriptions of the variables in `~/.openresearch/env`, by key.
    pub fn env_descriptions(&self) -> Result<std::collections::HashMap<String, String>> {
        let mut statement = self
            .conn
            .prepare("SELECT key, description FROM env_var_descriptions")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Describes a project's variable, or with no project one in
    /// `~/.openresearch/env`. An empty description clears it. With
    /// `only_if_missing`, an existing description is kept.
    pub fn describe_env(
        &self,
        project_id: Option<&str>,
        key: &str,
        description: &str,
        only_if_missing: bool,
    ) -> Result<()> {
        let description = description.trim();
        let description = (!description.is_empty()).then_some(description);
        let keep = if only_if_missing {
            " AND description IS NULL"
        } else {
            ""
        };
        match (project_id, description) {
            (Some(project), _) => {
                self.conn.execute(
                    &format!(
                        "UPDATE project_env_vars SET description = ?3
                         WHERE project_id = ?1 AND key = ?2{keep}"
                    ),
                    params![project, key, description],
                )?;
            }
            (None, Some(description)) => {
                let update = if only_if_missing {
                    "NOTHING"
                } else {
                    "UPDATE SET description = excluded.description"
                };
                self.conn.execute(
                    &format!(
                        "INSERT INTO env_var_descriptions (key, description) VALUES (?1, ?2)
                         ON CONFLICT (key) DO {update}"
                    ),
                    params![key, description],
                )?;
            }
            (None, None) if !only_if_missing => {
                self.conn
                    .execute("DELETE FROM env_var_descriptions WHERE key = ?1", [key])?;
            }
            (None, None) => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (Store, PathBuf) {
        let dir = std::env::temp_dir().join(format!("orx-store-env-{}", uuid::Uuid::new_v4()));
        (Store::open_at(dir.clone()).unwrap(), dir)
    }

    #[test]
    fn project_variables_keep_their_description_and_leave_with_the_project() {
        let (store, dir) = store();
        store
            .create_local_project(&super::super::tests::local_project_fixture("p"))
            .unwrap();
        store
            .set_project_env("p", "WANDB_PROJECT", "sweeps")
            .unwrap();
        store
            .describe_env(Some("p"), "WANDB_PROJECT", "Where runs log", true)
            .unwrap();
        store
            .describe_env(Some("p"), "WANDB_PROJECT", "ignored", true)
            .unwrap();
        store
            .set_project_env("p", "WANDB_PROJECT", "ablations")
            .unwrap();
        assert_eq!(
            store.project_env("p").unwrap(),
            [ProjectEnvVar {
                key: "WANDB_PROJECT".into(),
                value: "ablations".into(),
                description: Some("Where runs log".into()),
            }]
        );
        assert!(store.project_env("other").unwrap().is_empty());

        store.delete_local_project("p").unwrap();
        assert!(store.project_env("p").unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn global_descriptions_are_kept_replaced_and_cleared() {
        let (store, dir) = store();
        store
            .describe_env(None, "HF_TOKEN", "Hub token", true)
            .unwrap();
        store.describe_env(None, "HF_TOKEN", "other", true).unwrap();
        assert_eq!(store.env_descriptions().unwrap()["HF_TOKEN"], "Hub token");
        store
            .describe_env(None, "HF_TOKEN", "Read token", false)
            .unwrap();
        assert_eq!(store.env_descriptions().unwrap()["HF_TOKEN"], "Read token");
        store.describe_env(None, "HF_TOKEN", " ", false).unwrap();
        assert!(store.env_descriptions().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_run_gets_its_projects_variables_over_the_synced_ones() {
        use crate::local::trackio;

        if !crate::config::in_test_child(&[]) {
            return;
        }
        let key = format!("ORX_TEST_RUN_{}", uuid::Uuid::new_v4().simple());
        let other = format!("ORX_TEST_RUN_OTHER_{}", uuid::Uuid::new_v4().simple());
        let store = Store::open().unwrap();
        let run = super::super::tests::run_fixture(
            &format!("run_{}", uuid::Uuid::new_v4().simple()),
            "running",
            None,
        );
        store.upsert_run(&run).unwrap();
        let project = super::super::tests::local_project_fixture(&run.project_id);
        store.create_local_project(&project).unwrap();
        assert_eq!(
            crate::config::launch_env(&run).unwrap()[trackio::PROJECT],
            project.slug
        );
        crate::config::write_synced_env_vars(&[
            (&key, "global"),
            (&other, "global"),
            (trackio::PROJECT, "global-tracker"),
        ])
        .unwrap();
        assert_eq!(
            crate::config::launch_env(&run).unwrap()[trackio::PROJECT],
            "global-tracker"
        );
        store
            .set_project_env(&run.project_id, &key, "project")
            .unwrap();
        store
            .set_project_env(&run.project_id, trackio::PROJECT, "project-tracker")
            .unwrap();
        store
            .set_project_env(&run.project_id, crate::config::RUN_ID_ENV, "custom-id")
            .unwrap();

        let env = crate::config::launch_env(&run).unwrap();
        assert_eq!(env[&key], "project");
        assert_eq!(env[&other], "global");
        assert_eq!(env[trackio::PROJECT], "project-tracker");
        assert_eq!(env[crate::config::RUN_ID_ENV], run.id);
        assert_eq!(
            trackio::project_for(&store, project).unwrap().name,
            env[trackio::PROJECT]
        );
    }

    #[test]
    fn an_unreadable_project_store_stops_run_environment_resolution() {
        if !crate::config::in_test_child(&["ORX_DATA_DIR"]) {
            return;
        }
        crate::config::write_synced_env_var("ORX_TEST_RUN_ENV", "global").unwrap();
        let file = crate::config::test_home().join("not-a-directory");
        std::fs::write(&file, "").unwrap();
        std::env::set_var("ORX_DATA_DIR", file.join("data"));
        let run = super::super::tests::run_fixture("run_unreadable_env", "running", None);

        let error = crate::config::launch_env(&run).unwrap_err();
        assert!(error.to_string().contains(&run.id), "{error:#}");
        assert!(error.to_string().contains(&run.project_id), "{error:#}");
    }
}
