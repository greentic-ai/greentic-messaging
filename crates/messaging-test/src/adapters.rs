use std::env;

use anyhow::{Result, anyhow};
use serde_json::{Value, json};

use gsm_core::{MessageCard, OutKind, OutMessage, Platform, TenantCtx};
use gsm_translator::{
    TelegramTranslator, Translator, WebChatTranslator, slack::to_slack_payloads,
    teams::to_teams_adaptive, webex::to_webex_payload,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterMode {
    Real,
    DryRun,
}

pub struct AdapterPayload(pub Value);

pub struct SendResult {
    pub ok: bool,
    pub message_id: Option<String>,
    pub diagnostics: Option<String>,
}

pub trait AdapterSender: Send + Sync {
    fn translate(&self, ctx: &TenantCtx, card: &MessageCard) -> Result<AdapterPayload>;
    fn send(&self, ctx: &TenantCtx, payload: &AdapterPayload) -> Result<SendResult>;
}

pub struct AdapterTarget {
    pub name: &'static str,
    pub enabled: bool,
    pub mode: AdapterMode,
    pub reason: Option<String>,
    pub sender: Box<dyn AdapterSender>,
}

impl AdapterTarget {
    fn new(
        name: &'static str,
        enabled: bool,
        mode: AdapterMode,
        reason: Option<String>,
        sender: Box<dyn AdapterSender>,
    ) -> Self {
        Self {
            name,
            enabled,
            mode,
            reason,
            sender,
        }
    }
}

pub struct AdapterConfig {
    pub mode: AdapterMode,
}

impl AdapterConfig {
    pub fn load(force_dry_run: bool) -> Self {
        Self {
            mode: if force_dry_run {
                AdapterMode::DryRun
            } else {
                AdapterMode::Real
            },
        }
    }
}

pub fn registry_from_env(mode: AdapterMode) -> Vec<AdapterTarget> {
    let mut targets = Vec::new();
    targets.push(build_target(
        "teams",
        |ctx, card| {
            translate_card(ctx, card, Platform::Teams, |card, out| {
                to_teams_adaptive(card, out)
            })
        },
        mode,
        env::var("MS_GRAPH_TOKEN"),
    ));
    targets.push(build_target(
        "webex",
        |ctx, card| translate_card(ctx, card, Platform::Webex, |_, out| to_webex_payload(out)),
        mode,
        env::var("WEBEX_BOT_TOKEN"),
    ));
    targets.push(build_target(
        "slack",
        |ctx, card| {
            translate_card(ctx, card, Platform::Slack, |_, out| {
                next_value(to_slack_payloads(out))
            })
        },
        mode,
        env::var("SLACK_BOT_TOKEN"),
    ));
    targets.push(build_target(
        "webchat",
        |ctx, card| {
            translate_card(ctx, card, Platform::WebChat, |_, out| {
                next_value(WebChatTranslator::new().to_platform(out))
            })
        },
        mode,
        env::var("WEBCHAT_SECRET"),
    ));
    targets.push(build_target(
        "telegram",
        |ctx, card| {
            translate_card(ctx, card, Platform::Telegram, |_, out| {
                next_value(TelegramTranslator::new().to_platform(out))
            })
        },
        mode,
        env::var("TELEGRAM_BOT_TOKEN"),
    ));
    targets.push(build_target(
        "whatsapp",
        |ctx, card| {
            translate_card(ctx, card, Platform::WhatsApp, |_, _| {
                Ok(default_whatsapp_payload())
            })
        },
        mode,
        env::var("WHATSAPP_TOKEN"),
    ));
    targets
}

fn build_target(
    name: &'static str,
    translate: fn(&TenantCtx, &MessageCard) -> Result<AdapterPayload>,
    mode: AdapterMode,
    token: Result<String, env::VarError>,
) -> AdapterTarget {
    let enabled = token.is_ok();
    let reason = token.err().map(|err| format!("missing env: {err}"));
    AdapterTarget::new(
        name,
        enabled,
        mode,
        reason,
        Box::new(BasicAdapter { mode, translate }),
    )
}

struct BasicAdapter {
    mode: AdapterMode,
    translate: fn(&TenantCtx, &MessageCard) -> Result<AdapterPayload>,
}

impl AdapterSender for BasicAdapter {
    fn translate(&self, ctx: &TenantCtx, card: &MessageCard) -> Result<AdapterPayload> {
        (self.translate)(ctx, card)
    }

    fn send(&self, _ctx: &TenantCtx, _payload: &AdapterPayload) -> Result<SendResult> {
        match self.mode {
            AdapterMode::DryRun => Ok(SendResult {
                ok: true,
                message_id: Some("dry-run".into()),
                diagnostics: Some("dry run".into()),
            }),
            AdapterMode::Real => Err(anyhow!("real send not implemented")),
        }
    }
}

fn translate_card<F>(
    ctx: &TenantCtx,
    card: &MessageCard,
    platform: Platform,
    mapper: F,
) -> Result<AdapterPayload>
where
    F: Fn(&MessageCard, &OutMessage) -> Result<Value>,
{
    let chat_id = format!("fixture-{}", platform.as_str());
    let out = OutMessage {
        ctx: ctx.clone(),
        tenant: ctx.tenant.clone().into(),
        platform,
        chat_id,
        thread_id: None,
        kind: OutKind::Card,
        text: card.title.clone(),
        message_card: Some(card.clone()),
        adaptive_card: None,
        meta: Default::default(),
    };
    let value = mapper(card, &out)?;
    Ok(AdapterPayload(value))
}

fn next_value(list: Result<Vec<Value>>) -> Result<Value> {
    list?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no payloads produced"))
}

fn default_whatsapp_payload() -> Value {
    json!({
        "type": "whatsapp",
        "text": {
            "body": "fixture payload"
        }
    })
}
