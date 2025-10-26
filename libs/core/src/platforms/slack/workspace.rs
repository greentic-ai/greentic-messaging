use serde::{Deserialize, Serialize};

/// Stored credentials for a Slack workspace installation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackWorkspace {
    pub workspace_id: String,
    pub bot_token: String,
    #[serde(default)]
    pub enterprise_id: Option<String>,
}

impl SlackWorkspace {
    pub fn new(workspace_id: impl Into<String>, bot_token: impl Into<String>) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            bot_token: bot_token.into(),
            enterprise_id: None,
        }
    }
}

/// Directory-style index of installed workspaces for a tenant/team.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SlackWorkspaceIndex {
    #[serde(default)]
    pub workspaces: Vec<String>,
}

impl SlackWorkspaceIndex {
    /// Inserts a workspace id if it does not already exist.
    ///
    /// Returns `true` when the index was extended.
    pub fn insert(&mut self, workspace_id: &str) -> bool {
        if self
            .workspaces
            .iter()
            .any(|existing| existing == workspace_id)
        {
            return false;
        }
        self.workspaces.push(workspace_id.to_string());
        true
    }

    pub fn is_empty(&self) -> bool {
        self.workspaces.is_empty()
    }
}
