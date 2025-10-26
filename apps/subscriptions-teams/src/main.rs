use anyhow::{anyhow, Result};
use async_nats::Client as Nats;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::time::{sleep, Duration};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Clone, Deserialize, Serialize)]
struct AdminCmd {
    cmd: String,
    resource: Option<String>,
    secret: Option<String>,
    ttl_minutes: Option<u32>,
}

#[derive(Clone)]
struct Cfg {
    tenant: String,
    graph_tenant_id: String,
    client_id: String,
    client_secret: String,
    webhook_url: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let tenant = std::env::var("TENANT").unwrap_or_else(|_| "acme".into());
    let cfg = Cfg {
        tenant: tenant.clone(),
        graph_tenant_id: std::env::var("MS_GRAPH_TENANT_ID")?,
        client_id: std::env::var("MS_GRAPH_CLIENT_ID")?,
        client_secret: std::env::var("MS_GRAPH_CLIENT_SECRET")?,
        webhook_url: std::env::var("TEAMS_WEBHOOK_URL")?,
    };

    let nats = async_nats::connect(nats_url).await?;

    let admin_subject = format!("greentic.subs.admin.{}.teams", tenant);
    let mut admin = nats.subscribe(admin_subject.clone()).await?;
    tracing::info!("subscriptions-teams admin on {}", admin_subject);

    let renew_cfg = cfg.clone();
    let renew_nats = nats.clone();
    let renew_task = tokio::spawn(async move {
        if let Err(e) = renew_loop(renew_nats, renew_cfg).await {
            tracing::error!("renew loop exited: {e}");
        }
    });

    while let Some(msg) = admin.next().await {
        match serde_json::from_slice::<AdminCmd>(&msg.payload) {
            Ok(cmd) => {
                if let Err(e) = handle_admin(&nats, &cfg, cmd).await {
                    tracing::error!("admin err: {e}");
                }
            }
            Err(e) => tracing::warn!("invalid admin cmd: {e}"),
        }
    }

    renew_task.await.ok();
    Ok(())
}

async fn handle_admin(nats: &Nats, cfg: &Cfg, cmd: AdminCmd) -> Result<()> {
    match cmd.cmd.as_str() {
        "add" => add_subscription(nats, cfg, cmd).await?,
        "list" => list_subscriptions(nats, cfg).await?,
        "renew" => renew_matching(nats, cfg, cmd.resource.as_deref().unwrap_or("*")).await?,
        "delete" => delete_matching(nats, cfg, cmd.resource.as_deref().unwrap_or("*")).await?,
        other => tracing::warn!("unknown command {other}"),
    }
    Ok(())
}

async fn token(cfg: &Cfg) -> Result<String> {
    let url = format!(
        "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
        cfg.graph_tenant_id
    );
    let form = [
        ("client_id", cfg.client_id.as_str()),
        ("client_secret", cfg.client_secret.as_str()),
        ("grant_type", "client_credentials"),
        ("scope", "https://graph.microsoft.com/.default"),
    ];
    let res = reqwest::Client::new().post(url).form(&form).send().await?;
    let status = res.status();
    let bytes = res.bytes().await?;
    if !status.is_success() {
        let text = String::from_utf8_lossy(&bytes);
        return Err(anyhow!("token request failed: {} {}", status, text));
    }
    let v = parse_json(&bytes, "token")?;
    Ok(v["access_token"].as_str().unwrap_or_default().to_string())
}

async fn add_subscription(nats: &Nats, cfg: &Cfg, cmd: AdminCmd) -> Result<()> {
    let resource = cmd
        .resource
        .ok_or_else(|| anyhow::anyhow!("resource required"))?;
    let ttl = cmd.ttl_minutes.unwrap_or(55);
    let token = token(cfg).await?;

    let expires = (time::OffsetDateTime::now_utc() + time::Duration::minutes(ttl as i64))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into());
    let body = json!({
        "changeType": "created,updated",
        "notificationUrl": cfg.webhook_url,
        "resource": resource,
        "expirationDateTime": expires,
        "clientState": cmd.secret.unwrap_or_else(|| "greentic".into()),
    });

    let res = reqwest::Client::new()
        .post("https://graph.microsoft.com/v1.0/subscriptions")
        .bearer_auth(&token)
        .json(&body)
        .send()
        .await?;
    let status = res.status();
    let bytes = res.bytes().await?;
    if !status.is_success() {
        let text = String::from_utf8_lossy(&bytes);
        return Err(anyhow::anyhow!("graph add failed: {} {}", status, text));
    }
    let val = parse_json(&bytes, "graph add")?;
    publish_event(
        nats,
        &cfg.tenant,
        json!({
            "event": "created",
            "resource": resource,
            "response": val,
        }),
    )
    .await
}

async fn list_subscriptions(nats: &Nats, cfg: &Cfg) -> Result<()> {
    let token = token(cfg).await?;
    let res = reqwest::Client::new()
        .get("https://graph.microsoft.com/v1.0/subscriptions")
        .bearer_auth(&token)
        .send()
        .await?;
    let status = res.status();
    let bytes = res.bytes().await?;
    if !status.is_success() {
        let text = String::from_utf8_lossy(&bytes);
        return Err(anyhow::anyhow!("graph list failed: {} {}", status, text));
    }
    let val = parse_json(&bytes, "graph list")?;
    publish_event(
        nats,
        &cfg.tenant,
        json!({
            "event": "list",
            "response": val,
        }),
    )
    .await
}

async fn renew_matching(nats: &Nats, cfg: &Cfg, pattern: &str) -> Result<()> {
    let token = token(cfg).await?;
    let res = reqwest::Client::new()
        .get("https://graph.microsoft.com/v1.0/subscriptions")
        .bearer_auth(&token)
        .send()
        .await?;
    let status = res.status();
    let bytes = res.bytes().await?;
    if !status.is_success() {
        let text = String::from_utf8_lossy(&bytes);
        return Err(anyhow::anyhow!(
            "graph list (renew) failed: {} {}",
            status,
            text
        ));
    }
    let v = parse_json(&bytes, "graph list (renew)")?;
    if let Some(arr) = v.get("value").and_then(|x| x.as_array()) {
        for sub in arr {
            let id = sub.get("id").and_then(|x| x.as_str()).unwrap_or_default();
            let resource = sub.get("resource").and_then(|x| x.as_str()).unwrap_or("");
            if !glob_match(pattern, resource) {
                continue;
            }
            let exp = sub
                .get("expirationDateTime")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            if about_to_expire(exp, 15) {
                let new_exp = (time::OffsetDateTime::now_utc() + time::Duration::minutes(55))
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into());
                let body = json!({ "expirationDateTime": new_exp });
                let _ = reqwest::Client::new()
                    .patch(format!(
                        "https://graph.microsoft.com/v1.0/subscriptions/{}",
                        id
                    ))
                    .bearer_auth(&token)
                    .json(&body)
                    .send()
                    .await?;
                publish_event(
                    nats,
                    &cfg.tenant,
                    json!({
                        "event": "renewed",
                        "subscriptionId": id,
                        "resource": resource,
                    }),
                )
                .await?;
                tracing::info!("renewed subscription {}", id);
            }
        }
    }
    Ok(())
}

fn about_to_expire(exp: &str, window_minutes: i64) -> bool {
    if let Ok(dt) = time::OffsetDateTime::parse(exp, &time::format_description::well_known::Rfc3339)
    {
        let now = time::OffsetDateTime::now_utc();
        return dt - now < time::Duration::minutes(window_minutes);
    }
    false
}

async fn delete_matching(nats: &Nats, cfg: &Cfg, pattern: &str) -> Result<()> {
    let token = token(cfg).await?;
    let res = reqwest::Client::new()
        .get("https://graph.microsoft.com/v1.0/subscriptions")
        .bearer_auth(&token)
        .send()
        .await?;
    let status = res.status();
    let bytes = res.bytes().await?;
    if !status.is_success() {
        let text = String::from_utf8_lossy(&bytes);
        return Err(anyhow::anyhow!(
            "graph list (delete) failed: {} {}",
            status,
            text
        ));
    }
    let v = parse_json(&bytes, "graph list (delete)")?;
    if let Some(arr) = v.get("value").and_then(|x| x.as_array()) {
        for sub in arr {
            let id = sub.get("id").and_then(|x| x.as_str()).unwrap_or_default();
            let resource = sub.get("resource").and_then(|x| x.as_str()).unwrap_or("");
            if !glob_match(pattern, resource) {
                continue;
            }
            let _ = reqwest::Client::new()
                .delete(format!(
                    "https://graph.microsoft.com/v1.0/subscriptions/{}",
                    id
                ))
                .bearer_auth(&token)
                .send()
                .await?;
            publish_event(
                nats,
                &cfg.tenant,
                json!({
                    "event": "deleted",
                    "subscriptionId": id,
                    "resource": resource,
                }),
            )
            .await?;
            tracing::info!("deleted subscription {}", id);
        }
    }
    Ok(())
}

fn glob_match(pattern: &str, text: &str) -> bool {
    if pattern == "*" {
        true
    } else {
        text.contains(pattern)
    }
}

async fn publish_event(nats: &Nats, tenant: &str, payload: Value) -> Result<()> {
    let subject = format!("greentic.subs.events.{}.teams", tenant);
    let bytes = serde_json::to_vec(&payload)?;
    nats.publish(subject, bytes.into()).await?;
    Ok(())
}

async fn renew_loop(nats: Nats, cfg: Cfg) -> Result<()> {
    loop {
        if let Err(e) = renew_matching(&nats, &cfg, "*").await {
            tracing::error!("renew loop error: {e}");
        }
        sleep(Duration::from_secs(600)).await;
    }
}

fn parse_json(bytes: &[u8], context: &str) -> Result<Value> {
    serde_json::from_slice(bytes).map_err(|e| {
        anyhow!(
            "{context} response was not valid JSON: {e}; body={}",
            String::from_utf8_lossy(bytes)
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn about_to_expire_detects_window() {
        let exp = (time::OffsetDateTime::now_utc() + time::Duration::minutes(10))
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        assert!(about_to_expire(&exp, 15));
        assert!(!about_to_expire(&exp, 5));
    }

    #[test]
    fn glob_match_supports_wildcard_and_substrings() {
        assert!(glob_match("*", "/chats/123"));
        assert!(glob_match("chats", "/chats/123"));
        assert!(!glob_match("users", "/chats/123"));
    }

    #[test]
    fn parse_json_reports_context_in_error() {
        let err = parse_json(b"not json", "token").unwrap_err();
        assert!(format!("{err:?}").contains("token response"));
    }
}
