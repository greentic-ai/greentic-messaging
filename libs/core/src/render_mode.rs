//! Render mode abstraction to toggle between legacy rendering and the planner path.
use crate::{
    provider_capabilities::ProviderCapabilitiesV1,
    provider_registry::ProviderCapsRegistry,
    render_plan::{RenderPlan, RenderTier, RenderWarning},
    render_planner::{PlannerCard, plan_render, planner_policy},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RenderMode {
    #[default]
    Legacy,
    Planned,
}

/// Result of a render decision.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderOutcome {
    pub mode: RenderMode,
    pub plan: Option<RenderPlan>,
    pub warnings: Vec<RenderWarning>,
}

impl RenderOutcome {
    pub fn tier(&self) -> RenderTier {
        self.plan
            .as_ref()
            .map(|p| p.tier)
            .unwrap_or(RenderTier::TierD)
    }
}

/// Decide how to render based on render mode and provider capabilities.
///
/// Legacy mode bypasses the planner and returns no plan.
/// Planned mode pulls capabilities from the registry (or a supplied fallback) and runs the planner.
pub fn compute_render_outcome(
    mode: RenderMode,
    provider_id: &str,
    card: &PlannerCard,
    registry: &ProviderCapsRegistry,
    caps_fallback: Option<&ProviderCapabilitiesV1>,
) -> RenderOutcome {
    match mode {
        RenderMode::Legacy => RenderOutcome {
            mode,
            plan: None,
            warnings: Vec::new(),
        },
        RenderMode::Planned => {
            let caps = registry
                .get_caps(provider_id)
                .or(caps_fallback)
                .cloned()
                .unwrap_or_default();
            let plan = plan_render(card, &caps, &planner_policy());
            let warnings = plan.warnings.clone();
            RenderOutcome {
                mode,
                plan: Some(plan),
                warnings,
            }
        }
    }
}
