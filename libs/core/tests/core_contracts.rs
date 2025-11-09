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
        in_subject("acme", "teams", "chat/42"),
        "greentic.msg.in.acme.teams.chat-42"
    );
    assert_eq!(
        out_subject("acme", "telegram", "a b"),
        "greentic.msg.out.acme.telegram.a-b"
    );
    assert!(dlq_subject("out", "t", "p").starts_with("greentic.msg.dlq.out."));
    assert!(subs_subject("events", "t", "p").starts_with("greentic.subs.events."));
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
    let prev = std::env::var("GREENTIC_ENV").ok();
    unsafe {
        std::env::remove_var("GREENTIC_ENV");
    }
    let env = current_env();
    assert_eq!(env.as_str(), "dev");
    if let Some(prev) = prev {
        unsafe {
            std::env::set_var("GREENTIC_ENV", prev);
        }
    }
}

#[test]
fn provider_key_hash_matches() {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let key_a = ProviderKey {
        platform: Platform::Teams,
        env: EnvId("prod".into()),
        tenant: TenantId("acme".into()),
        team: Some(TeamId("alpha".into())),
    };
    let key_b = ProviderKey {
        platform: Platform::Teams,
        env: EnvId("prod".into()),
        tenant: TenantId("acme".into()),
        team: Some(TeamId("alpha".into())),
    };
    assert_eq!(key_a, key_b);

    let mut hasher_a = DefaultHasher::new();
    key_a.hash(&mut hasher_a);
    let mut hasher_b = DefaultHasher::new();
    key_b.hash(&mut hasher_b);
    assert_eq!(hasher_a.finish(), hasher_b.finish());

    let key_c = ProviderKey {
        team: Some(TeamId("beta".into())),
        ..key_a.clone()
    };
    assert_ne!(key_a, key_c);
}

#[test]
fn messaging_credentials_path_includes_env() {
    let _guard = env_lock().lock().unwrap();
    let prev = std::env::var("GREENTIC_ENV").ok();
    unsafe {
        std::env::set_var("GREENTIC_ENV", "test");
    }

    let ctx = make_tenant_ctx("acme".into(), Some("team-1".into()), Some("user-9".into()));
    let secret = messaging_credentials("telegram", &ctx);
    let rendered = secret.0.clone();
    assert!(
        rendered.contains("messaging"),
        "path missing messaging segment: {}",
        rendered
    );
    assert!(rendered.contains("/test/"));
    assert!(rendered.contains("/acme/"));
    assert!(rendered.contains("/team-1/"));

    if let Some(prev) = prev {
        unsafe {
            std::env::set_var("GREENTIC_ENV", prev);
        }
    } else {
        unsafe {
            std::env::remove_var("GREENTIC_ENV");
        }
    }
}

#[test]
fn slack_workspace_secret_includes_scope() {
    let ctx = make_tenant_ctx("acme".into(), Some("team-1".into()), None);
    let path = slack_workspace_secret(&ctx, "T123");
    let rendered = path.0;
    assert!(rendered.contains("/messaging/slack/"));
    assert!(rendered.contains("/acme/"));
    assert!(rendered.contains("/team-1/"));
    assert!(rendered.ends_with("/workspace/T123.json"));
}

#[test]
fn slack_workspace_index_includes_scope() {
    let ctx = make_tenant_ctx("acme".into(), Some("team-1".into()), None);
    let path = crate::slack_workspace_index(&ctx);
    let rendered = path.0;
    assert!(rendered.contains("/messaging/slack/"));
    assert!(rendered.contains("/acme/"));
    assert!(rendered.contains("/team-1/"));
    assert!(rendered.ends_with("/workspace/index.json"));
}

#[test]
fn teams_conversations_secret_includes_scope() {
    let ctx = make_tenant_ctx("acme".into(), Some("support".into()), None);
    let path = crate::teams_conversations_secret(&ctx);
    let rendered = path.0;
    assert!(rendered.contains("/messaging/teams/"));
    assert!(rendered.contains("/acme/"));
    assert!(rendered.contains("/support/"));
    assert!(rendered.ends_with("/conversations.json"));
}

#[test]
fn webex_credentials_path_includes_scope() {
    let ctx = make_tenant_ctx("acme".into(), Some("team-1".into()), None);
    let path = webex_credentials(&ctx);
    let rendered = path.0;
    assert!(rendered.contains("/messaging/webex/"));
    assert!(rendered.contains("/acme/"));
    assert!(rendered.contains("/team-1/"));
    assert!(rendered.ends_with("/credentials.json"));
}

#[test]
fn whatsapp_credentials_path_includes_scope() {
    let ctx = make_tenant_ctx("acme".into(), None, None);
    let path = whatsapp_credentials(&ctx);
    let rendered = path.0;
    assert!(rendered.contains("/messaging/whatsapp/"));
    assert!(rendered.contains("/acme/"));
    assert!(rendered.ends_with("/credentials.json"));
}
