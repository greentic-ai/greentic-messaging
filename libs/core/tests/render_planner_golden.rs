use gsm_core::{
    plan_render, planner_policy, provider_capabilities::ProviderCapabilitiesV1,
    render_plan::RenderTier, render_planner::PlannerCard,
};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
struct Fixture {
    #[serde(default)]
    capabilities: Option<FixtureCaps>,
    input: PlannerCard,
    expected: serde_json::Value,
}

#[derive(Debug, Deserialize, Default)]
struct FixtureCaps {
    #[serde(default)]
    supports_adaptive_cards: bool,
    #[serde(default)]
    supports_markdown: bool,
    #[serde(default)]
    supports_html: bool,
    #[serde(default)]
    supports_images: bool,
    #[serde(default)]
    supports_buttons: bool,
    #[serde(default)]
    supports_threads: bool,
    #[serde(default)]
    max_text_len: Option<u32>,
    #[serde(default)]
    max_payload_bytes: Option<u32>,
    #[serde(default)]
    max_actions: Option<u32>,
    #[serde(default)]
    max_buttons_per_row: Option<u32>,
    #[serde(default)]
    max_total_buttons: Option<u32>,
}

fn load_fixture(path: &Path) -> Fixture {
    let data = fs::read_to_string(path).expect("read fixture");
    serde_json::from_str(&data).expect("parse fixture json")
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .join("tests/fixtures/render_plan/v1")
}

#[test]
fn golden_fixtures_plan_render() {
    let fixtures_dir = fixtures_dir();
    for entry in fs::read_dir(fixtures_dir).expect("list fixtures") {
        let entry = entry.expect("fixture entry");
        if entry.file_type().map(|f| f.is_file()).unwrap_or(false) {
            if entry.file_name().to_string_lossy().starts_with("tier_") {
                continue;
            }
            let fixture = load_fixture(&entry.path());
            let caps = fixture
                .capabilities
                .as_ref()
                .map(fixture_caps_to_provider)
                .unwrap_or_default();
            let plan = plan_render(&fixture.input, &caps, &planner_policy());
            let json = serde_json::to_value(&plan).expect("plan to json");
            assert_eq!(
                json,
                fixture.expected,
                "fixture {} did not match",
                entry.path().display()
            );
        }
    }
}

#[derive(Debug, Deserialize)]
struct TierFixture {
    capabilities: TierCaps,
    input: PlannerCard,
    expected_tier: String,
    expected_warnings: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct TierCaps {
    #[serde(default)]
    supports_adaptive_cards: bool,
    #[serde(default)]
    supports_markdown: bool,
    #[serde(default)]
    supports_html: bool,
    #[serde(default)]
    supports_images: bool,
    #[serde(default)]
    supports_buttons: bool,
    #[serde(default)]
    supports_threads: bool,
    #[serde(default)]
    max_text_len: Option<u32>,
    #[serde(default)]
    max_payload_bytes: Option<u32>,
    #[serde(default)]
    max_actions: Option<u32>,
    #[serde(default)]
    max_buttons_per_row: Option<u32>,
    #[serde(default)]
    max_total_buttons: Option<u32>,
}

fn fixture_caps_to_provider(c: &FixtureCaps) -> ProviderCapabilitiesV1 {
    ProviderCapabilitiesV1 {
        version: "v1".into(),
        supports_adaptive_cards: c.supports_adaptive_cards,
        supports_markdown: c.supports_markdown,
        supports_html: c.supports_html,
        supports_images: c.supports_images,
        supports_buttons: c.supports_buttons,
        supports_threads: c.supports_threads,
        max_text_len: c.max_text_len,
        max_payload_bytes: c.max_payload_bytes,
        max_actions: c.max_actions,
        max_buttons_per_row: c.max_buttons_per_row,
        max_total_buttons: c.max_total_buttons,
        limits: Default::default(),
    }
}

fn tier_caps_to_provider(c: &TierCaps) -> ProviderCapabilitiesV1 {
    ProviderCapabilitiesV1 {
        version: "v1".into(),
        supports_adaptive_cards: c.supports_adaptive_cards,
        supports_markdown: c.supports_markdown,
        supports_html: c.supports_html,
        supports_images: c.supports_images,
        supports_buttons: c.supports_buttons,
        supports_threads: c.supports_threads,
        max_text_len: c.max_text_len,
        max_payload_bytes: c.max_payload_bytes,
        max_actions: c.max_actions,
        max_buttons_per_row: c.max_buttons_per_row,
        max_total_buttons: c.max_total_buttons,
        limits: Default::default(),
    }
}

#[test]
fn tier_selection_fixtures() {
    let fixtures_dir = fixtures_dir();
    let paths = [
        "tier_a_supported.json",
        "tier_a_downgrades_to_b.json",
        "tier_a_downgrades_to_d.json",
    ];
    for name in paths {
        let path = fixtures_dir.join(name);
        let data = fs::read_to_string(&path).expect("read tier fixture");
        let fixture: TierFixture = serde_json::from_str(&data).expect("parse tier fixture");

        let caps = tier_caps_to_provider(&fixture.capabilities);
        let plan = plan_render(&fixture.input, &caps, &planner_policy());
        let tier_name = match plan.tier {
            RenderTier::TierA => "tier_a",
            RenderTier::TierB => "tier_b",
            RenderTier::TierC => "tier_c",
            RenderTier::TierD => "tier_d",
        };
        assert_eq!(tier_name, fixture.expected_tier);
        let warnings = serde_json::to_value(&plan.warnings).expect("warnings to value");
        assert_eq!(
            warnings,
            serde_json::Value::Array(fixture.expected_warnings)
        );
    }
}
