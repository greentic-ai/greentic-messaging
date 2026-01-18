use gsm_core::*;
use std::sync::{Mutex, OnceLock};

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[test]
fn envelope_validates() {
    let env = MessageEnvelope {
        tenant: "acme".into(),
        platform: Platform::Telegram,
        chat_id: "room-1".into(),
        user_id: "u-9".into(),
        thread_id: None,
        msg_id: "msg-123".into(),
        text: Some("hi".into()),
        timestamp: "2025-10-14T09:00:00Z".into(),
        context: Default::default(),
    };
    assert!(validate_envelope(&env).is_ok());
}

#[test]
fn envelope_invalid_timestamp() {
    let mut env = MessageEnvelope {
        tenant: "acme".into(),
        platform: Platform::Slack,
        chat_id: "room-1".into(),
        user_id: "u-9".into(),
        thread_id: None,
        msg_id: "msg-123".into(),
        text: Some("hi".into()),
        timestamp: "bad-time".into(),
        context: Default::default(),
    };
    assert!(validate_envelope(&env).is_err());
    env.timestamp = "2025-10-14T10:00:00Z".into();
    assert!(validate_envelope(&env).is_ok());
}

#[test]
fn subjects_helpers_ok() {
    assert_eq!(
        ingress_subject("dev", "acme", "team", "slack"),
        "greentic.messaging.ingress.dev.acme.team.slack"
    );
    assert_eq!(
        egress_subject("dev", "acme", "team a", "web chat"),
        "greentic.messaging.egress.dev.acme.team-a.web-chat"
    );
}

#[test]
fn out_text_and_card_validate() {
    let ctx = make_tenant_ctx("acme".into(), None, None);
    let mut out = OutMessage {
        ctx: ctx.clone(),
        tenant: "acme".into(),
        platform: Platform::Teams,
        chat_id: "chat-1".into(),
        thread_id: None,
        kind: OutKind::Text,
        text: Some("hello".into()),
        message_card: None,
        #[cfg(feature = "adaptive-cards")]
        adaptive_card: None,
        meta: Default::default(),
    };
    assert!(validate_out(&out).is_ok());

    out.kind = OutKind::Card;
    out.text = None;
    out.message_card = Some(MessageCard {
        title: Some("Title".into()),
        body: vec![CardBlock::Text {
            text: "Body".into(),
            markdown: true,
        }],
        actions: vec![],
    });
    assert!(validate_out(&out).is_ok());
}

#[test]
fn current_env_defaults_to_dev() {
    let _guard = env_lock().lock().unwrap();
    let prev = current_env();
    set_current_env(EnvId::try_from("dev").expect("valid env id"));
    let env = current_env();
    assert_eq!(env.as_str(), "dev");
    set_current_env(prev);
}

#[test]
fn messaging_credentials_path_includes_env() {
    let _guard = env_lock().lock().unwrap();
    let prev = current_env();
    set_current_env(EnvId::try_from("test").expect("valid env id"));

    let ctx = make_tenant_ctx("acme".into(), Some("team-1".into()), Some("user-9".into()));
    let secret = messaging_credentials("telegram", &ctx);
    let rendered = secret.to_uri();
    assert!(
        rendered.contains("messaging"),
        "path missing messaging segment: {rendered}"
    );
    assert!(rendered.contains("/test/"));
    assert!(rendered.contains("/acme/"));
    assert!(rendered.contains("/team-1/"));

    set_current_env(prev);
}

#[test]
fn slack_workspace_secret_includes_scope() {
    let ctx = make_tenant_ctx("acme".into(), Some("team-1".into()), None);
    let path = slack_workspace_secret(&ctx, "T123");
    let rendered = path.to_uri();
    assert!(rendered.contains("messaging"));
    assert!(rendered.contains("/acme/"));
    assert!(rendered.contains("/team-1/"));
    assert!(rendered.contains("slack.workspace.t123.json"));
}

#[test]
fn slack_workspace_index_includes_scope() {
    let ctx = make_tenant_ctx("acme".into(), Some("team-1".into()), None);
    let path = crate::slack_workspace_index(&ctx);
    let rendered = path.to_uri();
    assert!(rendered.contains("messaging"));
    assert!(rendered.contains("/acme/"));
    assert!(rendered.contains("/team-1/"));
    assert!(rendered.contains("slack.workspace.index.json"));
}

#[test]
fn teams_conversations_secret_includes_scope() {
    let ctx = make_tenant_ctx("acme".into(), Some("support".into()), None);
    let path = crate::teams_conversations_secret(&ctx);
    let rendered = path.to_uri();
    assert!(rendered.contains("messaging"));
    assert!(rendered.contains("/acme/"));
    assert!(rendered.contains("/support/"));
    assert!(rendered.contains("teams.conversations.json"));
}

#[test]
fn webex_credentials_path_includes_scope() {
    let _guard = env_lock().lock().unwrap();
    let prev = current_env();
    set_current_env(EnvId::try_from("dev").expect("valid env id"));

    let ctx = make_tenant_ctx("acme".into(), Some("team-1".into()), None);
    let path = webex_credentials(&ctx);
    let rendered = path.uri().to_string();
    assert_eq!(
        rendered,
        "secrets://dev/acme/team-1/messaging/webex.credentials.json"
    );

    set_current_env(prev);
}

#[test]
fn whatsapp_credentials_path_includes_scope() {
    let ctx = make_tenant_ctx("acme".into(), None, None);
    let path = whatsapp_credentials(&ctx);
    let rendered = path.uri().to_string();
    assert_eq!(
        rendered,
        "secrets://dev/acme/_/messaging/whatsapp.credentials.json"
    );
}
