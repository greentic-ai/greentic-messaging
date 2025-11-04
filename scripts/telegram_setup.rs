use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use std::env;
use std::fs::OpenOptions;
use std::io::Write;

#[derive(Debug, Deserialize)]
struct ChatResponse {
    ok: bool,
    #[serde(default)]
    result: Option<Chat>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Chat {
    id: i64,
    #[allow(dead_code)]
    username: Option<String>,
    #[allow(dead_code)]
    title: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let mut args = env::args().skip(1);
    let mut handle: Option<String> = None;
    let mut token: Option<String> = None;
    let mut output: Option<String> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--handle" => handle = args.next(),
            "--token" => token = args.next(),
            "--output" => output = args.next(),
            _ => {}
        }
    }

    let token = token
        .or_else(|| env::var("TELEGRAM_BOT_TOKEN").ok())
        .context("TELEGRAM_BOT_TOKEN not set (pass --token or export the variable)")?;

    let handle = handle
        .or_else(|| env::var("TELEGRAM_CHAT_HANDLE").ok())
        .context("Chat handle missing (pass --handle or export TELEGRAM_CHAT_HANDLE)")?;

    let chat_id = resolve_chat_id(&token, &handle).await?;

    println!("Resolved {handle} → {chat_id}");

    if let Some(path) = output.or_else(|| env::var("TELEGRAM_ENV_PATH").ok()) {
        persist_chat_id(&path, chat_id)?;
        println!("Stored TELEGRAM_CHAT_ID in {path}");
    } else {
        println!("Add the following to your .env file:\nTELEGRAM_CHAT_ID={chat_id}");
    }

    Ok(())
}

async fn resolve_chat_id(token: &str, handle: &str) -> Result<i64> {
    let client = reqwest::Client::new();
    let url = format!("https://api.telegram.org/bot{token}/getChat");

    let response = client
        .post(url)
        .json(&serde_json::json!({ "chat_id": handle }))
        .send()
        .await
        .context("failed to call getChat")?;

    let status = response.status();
    let body: ChatResponse = response
        .json()
        .await
        .context("failed to decode getChat response")?;

    if !body.ok {
        return Err(anyhow!(
            "getChat returned error (status {status}): {:?}",
            body.description
        ));
    }

    let chat = body.result.context("getChat did not include chat data")?;

    Ok(chat.id)
}

fn persist_chat_id(path: &str, chat_id: i64) -> Result<()> {
    let line = format!("TELEGRAM_CHAT_ID={chat_id}\n");

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open {path}"))?;
    file.write_all(line.as_bytes())
        .with_context(|| format!("failed to write TELEGRAM_CHAT_ID to {path}"))?;
    Ok(())
}
