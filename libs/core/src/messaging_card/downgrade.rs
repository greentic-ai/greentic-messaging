use crate::messaging_card::ir::{Element, IrAction, MessageCardIr};
use crate::messaging_card::tier::Tier;
use tracing::warn;

#[derive(Debug, Clone)]
pub struct DowngradeContext {
    pub source: Tier,
    pub target: Tier,
    pub platform: Option<String>,
    pub profile: Option<CapabilityProfile>,
}

impl DowngradeContext {
    pub fn new(source: Tier, target: Tier) -> Self {
        Self {
            source,
            target,
            platform: None,
            profile: None,
        }
    }

    pub fn with_platform(mut self, platform: impl Into<String>) -> Self {
        self.platform = Some(platform.into());
        self
    }

    pub fn with_profile(mut self, profile: CapabilityProfile) -> Self {
        self.profile = Some(profile);
        self
    }
}

pub trait DowngradeEngine: Send + Sync {
    fn downgrade(&self, ir: &MessageCardIr, ctx: DowngradeContext) -> MessageCardIr;
}

#[derive(Debug, Default)]
pub struct PolicyDowngradeEngine;

impl DowngradeEngine for PolicyDowngradeEngine {
    fn downgrade(&self, ir: &MessageCardIr, ctx: DowngradeContext) -> MessageCardIr {
        if ctx.source == ctx.target {
            return ir.clone();
        }

        let mut downgraded = ir.clone();
        downgraded.tier = ctx.target;

        let profile = ctx
            .profile
            .unwrap_or_else(|| CapabilityProfile::for_tier(ctx.target));
        let platform = ctx.platform.unwrap_or_else(|| "generic".into());

        downgraded.elements = filter_elements(&ir.elements, &profile, &platform, &mut downgraded);
        downgraded.actions = filter_actions(&ir.actions, &profile, &platform, &mut downgraded);
        downgraded
            .meta
            .capabilities
            .retain(|cap| capability_allowed(&profile, cap));

        downgraded
    }
}

fn filter_elements(
    elements: &[Element],
    profile: &CapabilityProfile,
    platform: &str,
    ir: &mut MessageCardIr,
) -> Vec<Element> {
    elements
        .iter()
        .filter_map(|element| {
            if profile.supports_element(element) {
                Some(element.clone())
            } else {
                let descriptor = describe_element(element);
                warn!(
                    platform = %platform,
                    descriptor = %descriptor,
                    target_tier = ?ir.tier,
                    "downgrading removed unsupported element"
                );
                ir.meta
                    .warn(format!("Removed {descriptor} for {}", ir.tier.as_str()));
                None
            }
        })
        .collect()
}

fn filter_actions(
    actions: &[IrAction],
    profile: &CapabilityProfile,
    platform: &str,
    ir: &mut MessageCardIr,
) -> Vec<IrAction> {
    actions
        .iter()
        .filter_map(|action| {
            if profile.supports_action(action) {
                Some(action.clone())
            } else {
                let descriptor = describe_action(action);
                warn!(
                    platform = %platform,
                    descriptor = %descriptor,
                    target_tier = ?ir.tier,
                    "downgrading removed unsupported action"
                );
                ir.meta
                    .warn(format!("Removed {descriptor} for {}", ir.tier.as_str()));
                None
            }
        })
        .collect()
}

fn describe_element(element: &Element) -> &'static str {
    match element {
        Element::Text { .. } => "text",
        Element::Image { .. } => "image",
        Element::FactSet { .. } => "fact_set",
        Element::Input { .. } => "input",
    }
}

fn describe_action(action: &IrAction) -> &'static str {
    match action {
        IrAction::OpenUrl { .. } => "open_url",
        IrAction::Postback { .. } => "postback",
    }
}

fn capability_allowed(profile: &CapabilityProfile, cap: &str) -> bool {
    match cap {
        "inputs" | "execute" | "showcard" => profile.allow_inputs,
        "facts" => profile.allow_factset,
        _ => true,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityProfile {
    pub allow_images: bool,
    pub allow_factset: bool,
    pub allow_inputs: bool,
    pub allow_postbacks: bool,
}

impl CapabilityProfile {
    pub fn for_tier(tier: Tier) -> Self {
        match tier {
            Tier::Basic => Self {
                allow_images: false,
                allow_factset: false,
                allow_inputs: false,
                allow_postbacks: false,
            },
            Tier::Advanced => Self {
                allow_images: true,
                allow_factset: true,
                allow_inputs: false,
                allow_postbacks: true,
            },
            Tier::Premium => Self {
                allow_images: true,
                allow_factset: true,
                allow_inputs: true,
                allow_postbacks: true,
            },
        }
    }

    fn supports_element(&self, element: &Element) -> bool {
        match element {
            Element::Text { .. } => true,
            Element::Image { .. } => self.allow_images,
            Element::FactSet { .. } => self.allow_factset,
            Element::Input { .. } => self.allow_inputs,
        }
    }

    fn supports_action(&self, action: &IrAction) -> bool {
        match action {
            IrAction::OpenUrl { .. } => true,
            IrAction::Postback { .. } => self.allow_postbacks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging_card::ir::{Element, InputKind, MessageCardIrBuilder};
    use serde_json::json;

    #[test]
    fn basic_downgrade_removes_interactions() {
        let mut builder = MessageCardIrBuilder::default().tier(Tier::Premium);
        builder = builder.primary_text("hello", true);
        builder = builder.input(
            Some("Input".into()),
            InputKind::Text,
            Some("id".into()),
            Vec::new(),
        );
        builder = builder.postback("Ack", json!({}));
        let ir = builder.build();

        let ctx = DowngradeContext::new(Tier::Premium, Tier::Basic).with_platform("tests");
        let downgraded = PolicyDowngradeEngine::default().downgrade(&ir, ctx);
        assert_eq!(downgraded.tier, Tier::Basic);
        assert!(
            downgraded
                .elements
                .iter()
                .all(|el| matches!(el, Element::Text { .. }))
        );
        assert!(
            downgraded
                .actions
                .iter()
                .all(|action| matches!(action, IrAction::OpenUrl { .. }))
        );
        assert!(!downgraded.meta.warnings.is_empty());
    }

    #[test]
    fn advanced_profile_retains_rich_elements() {
        let mut builder = MessageCardIrBuilder::default().tier(Tier::Premium);
        builder = builder.primary_text("hello", false);
        builder = builder.image("https://example.com".into(), None);
        builder = builder.fact("Status", "Green");
        let ir = builder.build();

        let ctx = DowngradeContext::new(Tier::Premium, Tier::Advanced);
        let downgraded = PolicyDowngradeEngine::default().downgrade(&ir, ctx);
        assert_eq!(downgraded.elements.len(), 3);
        assert!(downgraded.meta.warnings.is_empty());
    }
}
