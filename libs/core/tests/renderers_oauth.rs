#![cfg(feature = "adaptive-cards")]

use gsm_core::PlatformRenderer;
use gsm_core::messaging_card::spec::{AuthRenderSpec, FallbackButton};
use gsm_core::messaging_card::types::{MessageCardKind, OauthCard, OauthPrompt, OauthProvider};
use gsm_core::messaging_card::{MessageCard, MessageCardEngine, TeamsRenderer, WebChatRenderer};
use serde_json::Value;

fn sample_auth_spec() -> AuthRenderSpec {
    AuthRenderSpec {
        provider: OauthProvider::Microsoft,
        scopes: vec!["User.Read".into()],
        resource: Some("https://graph.microsoft.com".into()),
        prompt: Some(OauthPrompt::Consent),
        metadata: None,
        start_url: Some("https://oauth.example/start".into()),
        connection_name: Some("m365".into()),
        fallback_button: FallbackButton {
            title: "Sign in with Microsoft".into(),
            url: Some("https://oauth.example/start".into()),
        },
    }
}

fn sample_oauth_card(connection_name: Option<&str>) -> MessageCard {
    MessageCard {
        kind: MessageCardKind::Oauth,
        title: Some("Sign in with Microsoft".into()),
        oauth: Some(OauthCard {
            provider: OauthProvider::Microsoft,
            scopes: vec!["User.Read".into()],
            resource: Some("https://graph.microsoft.com".into()),
            prompt: Some(OauthPrompt::Consent),
            metadata: None,
            start_url: Some("https://oauth.example/start".into()),
            connection_name: connection_name.map(|value| value.into()),
        }),
        ..Default::default()
    }
}

fn render_oauth_for(platform: &str) -> Value {
    let engine = MessageCardEngine::bootstrap();
    let card = sample_oauth_card(None);
    let spec = engine.render_spec(&card).expect("oauth spec");
    engine
        .render_spec_payload(platform, &spec)
        .unwrap_or_else(|| panic!("renderer {platform} missing"))
}

#[test]
fn teams_renders_native_oauth_card() {
    let renderer = TeamsRenderer;
    let auth = sample_auth_spec();
    let rendered = renderer
        .render_auth(&auth)
        .expect("teams supports oauth card");
    assert_eq!(rendered.payload, load_fixture("teams/oauth_native.json"));
}

#[test]
fn slack_oauth_downgrades_to_open_url() {
    let payload = render_oauth_for("slack");
    assert_eq!(payload, load_fixture("slack/oauth_downgrade.json"));
}

#[test]
fn telegram_oauth_downgrades_to_button() {
    let payload = render_oauth_for("telegram");
    assert_eq!(payload, load_fixture("telegram/oauth_downgrade.json"));
}

#[test]
fn webex_oauth_downgrades_to_card() {
    let payload = render_oauth_for("webex");
    assert_eq!(payload, load_fixture("webex/oauth_downgrade.json"));
}

#[test]
fn whatsapp_oauth_downgrades_to_template_link() {
    let payload = render_oauth_for("whatsapp");
    assert_eq!(payload, load_fixture("whatsapp/oauth_downgrade.json"));
}

#[test]
fn webchat_renders_native_oauth_card() {
    let renderer = WebChatRenderer;
    let auth = sample_auth_spec();
    let rendered = renderer
        .render_auth(&auth)
        .expect("webchat supports oauth card");
    assert_eq!(rendered.payload, load_fixture("webchat/oauth_native.json"));
}

fn load_fixture(path: &str) -> Value {
    let base = format!("tests/fixtures/renderers/{path}");
    let data = std::fs::read_to_string(base).expect("fixture missing");
    serde_json::from_str(&data).expect("invalid json")
}
