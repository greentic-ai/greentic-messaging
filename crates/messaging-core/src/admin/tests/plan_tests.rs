use crate::admin::plan::{ProvisionReportBuilder, ReportMode};

#[test]
fn builder_sets_plan_mode_and_tracks_entries() {
    let mut builder = ProvisionReportBuilder::new("mock", Some("tenant1"), ReportMode::Plan);
    builder.created("created-item");
    builder.updated("updated-item");
    builder.skipped("skipped-item");
    builder.warn("warn");
    builder.secret_written("messaging/global/mock/key");

    let report = builder.finish();
    assert_eq!(report.provider, "mock");
    assert_eq!(report.tenant.as_deref(), Some("tenant1"));
    assert_eq!(report.mode.as_deref(), Some("plan"));
    assert_eq!(report.created, vec!["created-item".to_string()]);
    assert_eq!(report.updated, vec!["updated-item".to_string()]);
    assert_eq!(report.skipped, vec!["skipped-item".to_string()]);
    assert_eq!(report.warnings, vec!["warn".to_string()]);
    assert_eq!(
        report.secret_keys_written,
        vec!["messaging/global/mock/key".to_string()]
    );
}

#[test]
fn builder_sets_ensure_mode() {
    let builder = ProvisionReportBuilder::new("mock", None, ReportMode::Ensure);
    let report = builder.finish();
    assert_eq!(report.mode.as_deref(), Some("ensure"));
    assert!(report.tenant.is_none());
}
