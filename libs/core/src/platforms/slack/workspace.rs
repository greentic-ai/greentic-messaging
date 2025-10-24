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
