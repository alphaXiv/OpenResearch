//! Wire-friendly local-mode entities — the same camelCase shapes the `orx up`
//! HTTP API serves. Row conversions live here beside the structs; the SQL
//! (matching column order) lives in `store.rs`.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalProject {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub github_owner: String,
    pub github_repo: String,
    pub github_sync_enabled: bool,
    pub github_auto_topics_enabled: bool,
    pub github_topics: Vec<String>,
    /// Fork point for baseline roots and the clone's default checkout — not
    /// where any experiment lives (legacy roots predating per-baseline
    /// branches may still ride it).
    pub baseline_branch: String,
    /// Local repository path.
    pub repo_path: String,
    pub run_command: Option<String>,
    /// arXiv id the project starts from (versionless, e.g. `2401.12345`).
    pub paper_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Baseline shape for a project row. Production code builds projects from an
/// explicit literal (all fields are meaningful there), but tests and the demo
/// seed only care about a few of them — having a default keeps a new column
/// from breaking every one of those literals.
impl Default for LocalProject {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            slug: String::new(),
            github_owner: String::new(),
            github_repo: String::new(),
            github_sync_enabled: false,
            github_auto_topics_enabled: false,
            github_topics: Vec::new(),
            baseline_branch: "main".to_string(),
            repo_path: String::new(),
            run_command: None,
            paper_id: None,
            created_at: 0,
            updated_at: 0,
        }
    }
}

impl LocalProject {
    /// Test-only project literal; every field the tests do not set takes its
    /// default. Keeps new schema columns from rippling through fixtures.
    #[cfg(test)]
    pub fn for_test(id: &str) -> Self {
        Self {
            id: id.to_string(),
            name: id.to_string(),
            slug: id.to_string(),
            ..Self::default()
        }
    }

    pub fn github_enabled(&self) -> bool {
        self.github_sync_enabled && self.has_github_repository()
    }

    pub fn has_github_repository(&self) -> bool {
        !self.github_owner.trim().is_empty() && !self.github_repo.trim().is_empty()
    }

    pub fn github_url(&self) -> Option<String> {
        self.has_github_repository().then(|| {
            format!(
                "https://github.com/{}/{}",
                self.github_owner, self.github_repo
            )
        })
    }

    pub fn github_topics_enabled(&self) -> bool {
        self.github_enabled() && self.github_auto_topics_enabled
    }

    /// Column order must match `store::PROJECT_COLS`.
    pub(crate) fn from_row(row: &rusqlite::Row<'_>) -> std::result::Result<Self, rusqlite::Error> {
        let github_topics: String = row.get(7)?;
        Ok(Self {
            id: row.get(0)?,
            name: row.get(1)?,
            slug: row.get(2)?,
            github_owner: row.get(3)?,
            github_repo: row.get(4)?,
            github_sync_enabled: row.get(5)?,
            github_auto_topics_enabled: row.get(6)?,
            github_topics: serde_json::from_str::<Vec<String>>(&github_topics).unwrap_or_default(),
            baseline_branch: row.get(8)?,
            repo_path: row.get(9)?,
            run_command: row.get(10)?,
            paper_id: row.get(11)?,
            created_at: row.get(12)?,
            updated_at: row.get(13)?,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalExperiment {
    pub id: String,
    pub project_id: String,
    /// NULL = baseline/root.
    pub parent_experiment_id: Option<String>,
    pub slug: String,
    /// `orx/<slug>` (legacy baselines ride the project's baseline branch).
    pub branch_name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub run_command: String,
    pub agent_status: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// Chat session that created this experiment. NULL for dashboard-created,
    /// legacy and out-of-session rows. Immutable once stamped.
    pub chat_session_id: Option<String>,
}

impl LocalExperiment {
    /// Column order must match `store::EXPERIMENT_COLS`.
    pub(crate) fn from_row(row: &rusqlite::Row<'_>) -> std::result::Result<Self, rusqlite::Error> {
        Ok(Self {
            id: row.get(0)?,
            project_id: row.get(1)?,
            parent_experiment_id: row.get(2)?,
            slug: row.get(3)?,
            branch_name: row.get(4)?,
            title: row.get(5)?,
            description: row.get(6)?,
            run_command: row.get(7)?,
            agent_status: row.get(8)?,
            created_at: row.get(9)?,
            updated_at: row.get(10)?,
            chat_session_id: row.get(11)?,
        })
    }

    /// Display name: title when set, slug otherwise.
    pub fn display_name(&self) -> &str {
        match self.title.as_deref() {
            Some(t) if !t.trim().is_empty() => t,
            _ => &self.slug,
        }
    }
}
