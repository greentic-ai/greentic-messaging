use gsm_core::{
    compute_render_outcome,
    provider_capabilities::ProviderCapabilitiesV1,
    provider_registry::{CapsSource, ProviderCapsRegistry},
    render_mode::RenderMode,
    render_plan::RenderTier,
    render_planner::PlannerCard,
};

#[test]
fn legacy_mode_skips_planner() {
    let registry = ProviderCapsRegistry::new();
    let card = PlannerCard {
        title: Some("Hello".into()),
        text: Some("World".into()),
        actions: vec![],
        images: vec![],
    };

    let outcome = compute_render_outcome(RenderMode::Legacy, "provider-x", &card, &registry, None);

    assert!(outcome.plan.is_none());
    assert_eq!(outcome.warnings.len(), 0);
    assert_eq!(outcome.tier(), RenderTier::TierD);
}

#[test]
fn planned_mode_uses_registry_caps() {
    let mut registry = ProviderCapsRegistry::new();
    let caps = ProviderCapabilitiesV1 {
        supports_adaptive_cards: true,
        ..Default::default()
    };
    registry.register_provider("provider-y", "0.1.0", CapsSource::Override, caps, None);
    let card = PlannerCard {
        title: Some("Hello".into()),
        text: Some("World".into()),
        actions: vec![],
        images: vec![],
    };

    let outcome = compute_render_outcome(RenderMode::Planned, "provider-y", &card, &registry, None);

    let plan = outcome.plan.expect("plan exists");
    assert_eq!(plan.tier, RenderTier::TierA);
    assert!(outcome.warnings.is_empty());
}

#[test]
fn planned_mode_falls_back_to_default_caps() {
    let registry = ProviderCapsRegistry::new();
    let card = PlannerCard {
        title: Some("Title".into()),
        text: None,
        actions: vec![],
        images: vec!["img".into()],
    };
    let fallback_caps = ProviderCapabilitiesV1::default();
    let outcome = compute_render_outcome(
        RenderMode::Planned,
        "unknown",
        &card,
        &registry,
        Some(&fallback_caps),
    );

    // default caps do not support images, so expect TierD warning path
    assert_eq!(outcome.tier(), RenderTier::TierD);
    assert!(!outcome.warnings.is_empty());
}
