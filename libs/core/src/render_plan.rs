//! Minimal render-planning types shared across renderers.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Render tiers are coarse buckets for capability and downgrade decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RenderTier {
    TierA,
    TierB,
    TierC,
    TierD,
}

/// A renderer-independent plan describing what to render and any warnings produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RenderPlan {
    /// Target capability tier.
    pub tier: RenderTier,
    /// Human-friendly summary text (optional).
    pub summary_text: Option<String>,
    /// Action identifiers/labels the host can map to platform-specific actions.
    pub actions: Vec<String>,
    /// Attachment references (URLs or opaque identifiers) to include in the render.
    pub attachments: Vec<String>,
    /// Deterministic warnings emitted during planning.
    pub warnings: Vec<RenderWarning>,
    /// Opaque debug payload for diagnostics.
    pub debug: Option<Value>,
}

/// Structured warning emitted during render planning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RenderWarning {
    /// Machine-readable warning code (e.g., "text_truncated").
    pub code: String,
    /// Optional human-readable description.
    pub message: Option<String>,
    /// Optional JSON pointer–like path indicating where the warning applies.
    pub path: Option<String>,
}
