//! WhatsApp egress adapter. Sends text messages when within the 24-hour session
//! window and falls back to approved templates when required.

use anyhow::{Result, anyhow};
use async_nats::jetstream::AckKind;
use futures::StreamExt;
use gsm_backpressure::BackpressureLimiter;
use gsm_core::egress::{EgressSender, OutboundMessage};
use gsm_core::messaging_card::{MessageCardKind, ensure_oauth_start_url};
use gsm_core::oauth::OauthClient;
use gsm_core::platforms::whatsapp::{WhatsAppCreds, WhatsAppSender};
use gsm_core::prelude::{DefaultResolver, SecretsResolver};
use gsm_core::telemetry::install as init_telemetry;
use gsm_core::{OutKind, OutMessage, Platform, TenantCtx};
use gsm_dlq::{DlqError, DlqPublisher};
use gsm_egress_common::{
    egress::bootstrap,
    telemetry::{
        AuthRenderMode, context_from_out, record_auth_card_render, record_egress_success,
        start_acquire_span, start_send_span,
    },
};
use gsm_translator::secure_action_url;
use serde_json::json;
use std::sync::Arc;
use std::time::Instant;
use time::{Duration, OffsetDateTime};
use tracing::{Instrument, Level, event};

const SESSION_WINDOW_HOURS: i64 = 24;

#[derive(Clone)]
struct AppConfig {
    template_name: String,
    template_lang: String,
    api_base: String,
    fallback_url: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_telemetry("greentic-messaging")?;
    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let tenant = std::env::var("TENANT").unwrap_or_else(|_| "acme".into());
    let template_name =
        std::env::var("WA_TEMPLATE_NAME").unwrap_or_else(|_| "weather_update".into());
    let template_lang = std::env::var("WA_TEMPLATE_LANG").unwrap_or_else(|_| "en".into());
    #[cfg(feature = "mock-http")]
    // Tests rely on bypassing Meta's Graph API, so we force a mock endpoint here.
    let api_base = "mock://wa".to_string();
    #[cfg(not(feature = "mock-http"))]
    let api_base =
        std::env::var("WA_API_BASE").unwrap_or_else(|_| "https://graph.facebook.com/v19.0".into());
    let fallback_url =
        std::env::var("WA_FALLBACK_URL").unwrap_or_else(|_| "https://app.greentic.ai".into());

    let config = AppConfig {
        template_name,
        template_lang,
        api_base: api_base.clone(),
        fallback_url,
    };

    let queue = bootstrap(&nats_url, &tenant, Platform::WhatsApp.as_str()).await?;
    tracing::info!(
        stream = %queue.stream,
        consumer = %queue.consumer,
        "egress-whatsapp consuming from JetStream"
    );

    let client = queue.client();
    let mut messages = queue.messages;
    let limiter = queue.limiter;
    let dlq = DlqPublisher::new("egress", client).await?;
    let resolver = Arc::new(DefaultResolver::new().await?);
    let http = reqwest::Client::new();
    let sender = Arc::new(WhatsAppSender::new(http.clone(), resolver, Some(api_base)));
    let oauth_client = match std::env::var("OAUTH_BASE_URL") {
        Ok(_) => match OauthClient::from_env(http.clone()) {
            Ok(client) => {
                tracing::info!("OAUTH_BASE_URL detected; WhatsApp OAuth builder enabled");
                Some(Arc::new(client))
            }
            Err(err) => {
                tracing::warn!(error = %err, "failed to initialize WhatsApp OAuth client");
                None
            }
        },
        Err(_) => None,
    };

    while let Some(next) = messages.next().await {
        let msg = match next {
            Ok(msg) => msg,
            Err(err) => {
                tracing::error!("jetstream message error: {err}");
                continue;
            }
        };

        let mut out: OutMessage = match serde_json::from_slice(msg.payload.as_ref()) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("bad out msg: {e}");
                let _ = msg.ack().await;
                continue;
            }
        };

        if out.platform != Platform::WhatsApp {
            if let Err(err) = msg.ack().await {
                tracing::error!("ack failed: {err}");
            }
            continue;
        }

        let mut drop_adaptive = false;
        if let (Some(card), Some(client)) = (out.adaptive_card.as_mut(), oauth_client.as_deref())
            && let Err(err) = ensure_oauth_start_url(card, &out.ctx, client, None).await
        {
            tracing::warn!(
                error = %err,
                platform = "whatsapp",
                "failed to build oauth start_url; downgrading"
            );
            drop_adaptive = true;
        }
        if drop_adaptive {
            out.adaptive_card = None;
        }

        let ctx = context_from_out(&out);
        if let Some(card) = out.adaptive_card.as_ref()
            && matches!(card.kind, MessageCardKind::Oauth)
            && let Some(oauth) = card.oauth.as_ref()
        {
            let team = out.ctx.team.as_ref().map(|team| team.as_ref());
            record_auth_card_render(
                &ctx,
                oauth.provider.as_str(),
                AuthRenderMode::Downgrade,
                oauth.connection_name.as_deref(),
                oauth.start_url.as_deref(),
                team,
            );
        }
        let msg_id = ctx
            .labels
            .msg_id
            .clone()
            .unwrap_or_else(|| out.message_id());
        let acquire_span = start_acquire_span(&ctx);
        let _permit = match limiter.acquire(&out.tenant).instrument(acquire_span).await {
            Ok(p) => p,
            Err(err) => {
                tracing::error!(
                    error = %err,
                    tenant = %ctx.labels.tenant,
                    platform = "whatsapp",
                    "failed to acquire backpressure permit"
                );
                let _ = msg.ack_with(AckKind::Nak(None)).await;
                continue;
            }
        };
        event!(
            Level::INFO,
            tenant = %ctx.labels.tenant,
            platform = "whatsapp",
            msg_id = %msg_id,
            acquired = true,
            "backpressure permit acquired"
        );

        let send_start = Instant::now();
        let send_span = start_send_span(&ctx);
        let result = dispatch_message(&http, sender.as_ref(), &config, &out)
            .instrument(send_span)
            .await;
        let elapsed_ms = send_start.elapsed().as_secs_f64() * 1000.0;

        match result {
            Ok(()) => {
                if let Err(err) = msg.ack().await {
                    tracing::error!("ack failed: {err}");
                }
                record_egress_success(&ctx, elapsed_ms);
            }
            Err(e) => {
                tracing::warn!("failed to send whatsapp message: {e}");
                if let Err(err) = dlq
                    .publish(
                        &out.tenant,
                        out.platform.as_str(),
                        &msg_id,
                        1,
                        DlqError {
                            code: "E_SEND".into(),
                            message: e.to_string(),
                            stage: None,
                        },
                        &out,
                    )
                    .await
                {
                    tracing::error!("failed to publish dlq entry: {err}");
                }
                if let Err(err) = msg.ack_with(AckKind::Nak(None)).await {
                    tracing::error!("nak failed: {err}");
                }
            }
        }
    }

    Ok(())
}

async fn dispatch_message<R>(
    http: &reqwest::Client,
    sender: &WhatsAppSender<R>,
    cfg: &AppConfig,
    out: &OutMessage,
) -> Result<()>
where
    R: SecretsResolver + Send + Sync,
{
    let chat_id = out.chat_id.clone();
    let msg_id = out.message_id();

    enum Dispatch {
        Text { text: String },
        Fallback { text: String },
    }

    let decision = {
        let translate_span = tracing::info_span!(
            "translate.run",
            env = %out.ctx.env.as_str(),
            tenant = %out.tenant,
            platform = %out.platform.as_str(),
            chat_id = %chat_id,
            msg_id = %msg_id
        );
        let _guard = translate_span.enter();
        match out.kind {
            OutKind::Text => {
                let text = out.text.clone().unwrap_or_default();
                if within_session_window(out) {
                    Dispatch::Text { text }
                } else {
                    tracing::info!("session window expired; sending template fallback");
                    Dispatch::Fallback { text }
                }
            }
            OutKind::Card => Dispatch::Fallback {
                text: String::new(),
            },
        }
    };

    match decision {
        Dispatch::Text { text } => send_text(sender, &out.ctx, &chat_id, &text).await,
        Dispatch::Fallback { text } => {
            send_card_fallback(http, sender, cfg, &out.ctx, out, &chat_id, &text).await
        }
    }
}

fn within_session_window(out: &OutMessage) -> bool {
    let last_interacted = out
        .meta
        .get("wa_last_interaction")
        .and_then(|v| v.as_str())
        .and_then(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok())
        .unwrap_or_else(OffsetDateTime::now_utc);

    OffsetDateTime::now_utc() - last_interacted <= Duration::hours(SESSION_WINDOW_HOURS)
}

struct FallbackLink {
    title: String,
    url: String,
    is_oauth: bool,
}

fn determine_fallback_link(cfg: &AppConfig, out: &OutMessage, text: &str) -> FallbackLink {
    if let Some(card) = &out.adaptive_card
        && matches!(card.kind, MessageCardKind::Oauth)
        && let Some(oauth) = &card.oauth
        && let Some(start_url) = &oauth.start_url
    {
        let title = card
            .title
            .clone()
            .unwrap_or_else(|| format!("Sign in with {}", oauth.provider.display_name()));
        return FallbackLink {
            title,
            url: start_url.clone(),
            is_oauth: true,
        };
    }

    let title = out
        .message_card
        .as_ref()
        .and_then(|c| c.title.clone())
        .or_else(|| out.adaptive_card.as_ref().and_then(|c| c.title.clone()))
        .unwrap_or_else(|| text.to_string());

    let url = secure_action_url(out, "fallback", &cfg.fallback_url);
    FallbackLink {
        title,
        url,
        is_oauth: false,
    }
}

async fn send_card_fallback<R>(
    http: &reqwest::Client,
    sender: &WhatsAppSender<R>,
    cfg: &AppConfig,
    ctx: &TenantCtx,
    out: &OutMessage,
    chat_id: &str,
    text: &str,
) -> Result<()>
where
    R: SecretsResolver + Send + Sync,
{
    let fallback = determine_fallback_link(cfg, out, text);
    let mut vars = Vec::new();
    if !fallback.title.is_empty() {
        vars.push(fallback.title.as_str());
    }
    vars.push(fallback.url.as_str());

    let creds = sender
        .credentials(ctx)
        .await
        .map_err(|err| anyhow!(err.to_string()))?;

    match send_template(http, cfg, &creds, ctx, chat_id, vars.as_slice()).await {
        Ok(()) => Ok(()),
        Err(e) => {
            tracing::warn!("template send failed, falling back to text: {e}");
            let fallback_text = if fallback.is_oauth {
                if fallback.title.is_empty() {
                    format!("Sign in: {}", fallback.url)
                } else {
                    format!("{} — {}", fallback.title, fallback.url)
                }
            } else if text.is_empty() {
                format!("View details: {}", fallback.url)
            } else {
                format!("{} — {}", text, fallback.url)
            };
            send_text(sender, ctx, chat_id, &fallback_text).await
        }
    }
}

async fn send_text<R>(
    sender: &WhatsAppSender<R>,
    ctx: &TenantCtx,
    to: &str,
    body: &str,
) -> Result<()>
where
    R: SecretsResolver + Send + Sync,
{
    sender
        .send(
            ctx,
            OutboundMessage {
                channel: Some(to.to_string()),
                text: Some(body.to_string()),
                payload: None,
            },
        )
        .await
        .map(|_| ())
        .map_err(|err| anyhow!(err.to_string()))
}

async fn send_template(
    http: &reqwest::Client,
    cfg: &AppConfig,
    creds: &WhatsAppCreds,
    ctx: &TenantCtx,
    to: &str,
    variables: &[&str],
) -> Result<()> {
    let url = format!(
        "{}/{}/messages",
        cfg.api_base.trim_end_matches('/'),
        creds.phone_id
    );
    let body = json!({
        "messaging_product": "whatsapp",
        "to": to,
        "type": "template",
        "template": {
          "name": cfg.template_name,
          "language": { "code": cfg.template_lang },
          "components": [
            {
              "type": "body",
              "parameters": variables.iter().map(|v| json!({
                "type": "text",
                "text": v
              })).collect::<Vec<_>>()
            }
          ]
        }
    });

    let response = http
        .post(url)
        .bearer_auth(&creds.user_token)
        .json(&body)
        .send()
        .await?;
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(anyhow!(
            "wa template err (env={}, tenant={}) {} {}",
            ctx.env.as_str(),
            ctx.tenant.as_str(),
            status,
            text
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gsm_core::make_tenant_ctx;
    use gsm_core::messaging_card::{
        MessageCard as AdaptiveCard, MessageCardKind, OauthCard, OauthProvider,
    };

    fn sample_message(timestamp_offset_hours: i64) -> OutMessage {
        let last = OffsetDateTime::now_utc() - Duration::hours(timestamp_offset_hours);
        let mut meta = serde_json::Map::new();
        meta.insert(
            "wa_last_interaction".into(),
            json!(
                last.format(&time::format_description::well_known::Rfc3339)
                    .unwrap()
            ),
        );
        OutMessage {
            ctx: make_tenant_ctx("acme".into(), None, None),
            tenant: "acme".into(),
            platform: Platform::WhatsApp,
            chat_id: "12345".into(),
            thread_id: None,
            kind: OutKind::Text,
            text: Some("Hello".into()),
            message_card: None,
            adaptive_card: None,
            meta: meta.into_iter().collect(),
        }
    }

    #[test]
    fn within_session_window_true() {
        let out = sample_message(1);
        assert!(within_session_window(&out));
    }

    #[test]
    fn within_session_window_false() {
        let out = sample_message(48);
        assert!(!within_session_window(&out));
    }

    #[test]
    fn send_template_builds_body() {
        let cfg = AppConfig {
            template_name: "weather".into(),
            template_lang: "en".into(),
            api_base: "https://graph.facebook.com/v19.0/".into(),
            fallback_url: "https://app.greentic.ai".into(),
        };
        let creds = WhatsAppCreds {
            phone_id: "123".into(),
            user_token: "token".into(),
        };
        let url = format!(
            "{}/{}/messages",
            cfg.api_base.trim_end_matches('/'),
            creds.phone_id
        );
        assert_eq!(url, "https://graph.facebook.com/v19.0/123/messages");
    }

    fn oauth_message(start_url: &str) -> OutMessage {
        let mut out = sample_message(48);
        out.kind = OutKind::Card;
        let card = AdaptiveCard {
            kind: MessageCardKind::Oauth,
            title: Some("Sign in with Microsoft".into()),
            oauth: Some(OauthCard {
                provider: OauthProvider::Microsoft,
                scopes: Vec::new(),
                resource: None,
                prompt: None,
                start_url: Some(start_url.into()),
                connection_name: None,
                metadata: None,
            }),
            ..Default::default()
        };
        out.adaptive_card = Some(card);
        out
    }

    #[test]
    fn fallback_prefers_oauth_start_url() {
        let cfg = AppConfig {
            template_name: "weather".into(),
            template_lang: "en".into(),
            api_base: "https://graph.facebook.com/v19.0/".into(),
            fallback_url: "https://app.greentic.ai".into(),
        };
        let out = oauth_message("https://oauth.example/start");
        let link = determine_fallback_link(&cfg, &out, "");
        assert_eq!(link.url, "https://oauth.example/start");
        assert!(link.is_oauth);
        assert!(link.title.contains("Microsoft"));
    }
}
