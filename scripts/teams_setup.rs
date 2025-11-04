use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use std::env;
use std::fs::OpenOptions;
use std::io::Write;

#[derive(Debug, Deserialize)]
struct ChatInfo {
    id: Option<String>,
    topic: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let mut args = env::args().skip(1);
    let mut tenant = None;
    let mut client_id = None;
    let mut client_secret = None;
    let mut chat_id = None;
    let mut output = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--tenant" => tenant = args.next(),
            "--client-id" => client_id = args.next(),
            "--client-secret" => client_secret = args.next(),
            "--chat-id" => chat_id = args.next(),
            "--output" => output = args.next(),
            _ => {}
        }
    }

    let tenant_id = tenant
        .or_else(|| env::var("TEAMS_TENANT_ID").ok())
        .context("TEAMS_TENANT_ID missing (provide --tenant)")?;
    let client_id = client_id
        .or_else(|| env::var("TEAMS_CLIENT_ID").ok())
        .context("TEAMS_CLIENT_ID missing (provide --client-id)")?;
    let client_secret = client_secret
        .or_else(|| env::var("TEAMS_CLIENT_SECRET").ok())
        .context("TEAMS_CLIENT_SECRET missing (provide --client-secret)")?;
    let chat_id = chat_id
        .or_else(|| env::var("TEAMS_CHAT_ID").ok())
        .context("TEAMS_CHAT_ID missing (provide --chat-id)")?;

    let client = reqwest::Client::new();
    let token = acquire_token(&client, &tenant_id, &client_id, &client_secret).await?;

    let chat = client
        .get(format!("https://graph.microsoft.com/v1.0/chats/{chat_id}"))
        .bearer_auth(&token)
        .send()
        .await
        .context("failed to fetch chat")?
        .error_for_status()
        .map_err(|err| anyhow!("chat lookup failed: {err}"))?
        .json::<ChatInfo>()
        .await
        .context("failed to decode chat info")?;

    println!(
        "Verified chat {} (topic: {})",
        chat.id.unwrap_or_else(|| chat_id.clone()),
        chat.topic.unwrap_or_else(|| "(no topic)".into())
    );

    if let Some(path) = output.or_else(|| env::var("TEAMS_ENV_PATH").ok()) {
        persist_env(&path, &tenant_id, &client_id, &client_secret, &chat_id)?;
        println!("Stored TEAMS_* secrets in {path}");
    } else {
        println!("Add to your .env:");
        println!("TEAMS_TENANT_ID={tenant_id}");
        println!("TEAMS_CLIENT_ID={client_id}");
        println!("TEAMS_CLIENT_SECRET={client_secret}");
        println!("TEAMS_CHAT_ID={chat_id}");
    }

    Ok(())
}

async fn acquire_token(
    client: &reqwest::Client,
    tenant_id: &str,
    client_id: &str,
    client_secret: &str,
) -> Result<String> {
    #[derive(Deserialize)]
    struct Token { access_token: String }

    let res = client
        .post(format!(
            "https://login.microsoftonline.com/{tenant_id}/oauth2/v2.0/token"
        ))
        .form(&[
            ("grant_type", "client_credentials"),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("scope", "https://graph.microsoft.com/.default"),
        ])
        .send()
        .await
        .context("token request failed")?
        .error_for_status()
        .map_err(|err| anyhow!("token request failed: {err}"))?
        .json::<Token>()
        .await
        .context("failed to decode token response")?;

    Ok(res.access_token)
}

fn persist_env(path: &str, tenant: &str, client_id: &str, client_secret: &str, chat_id: &str) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open {path}"))?;

    writeln!(file, "TEAMS_TENANT_ID={tenant}")?;
    writeln!(file, "TEAMS_CLIENT_ID={client_id}")?;
    writeln!(file, "TEAMS_CLIENT_SECRET={client_secret}")?;
    writeln!(file, "TEAMS_CHAT_ID={chat_id}")?;

    Ok(())
}
