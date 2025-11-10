use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use std::env;
use std::fs::OpenOptions;
use std::io::Write;

#[derive(Debug, Deserialize)]
struct PhoneInfo {
    #[serde(default)]
    display_phone_number: Option<String>,
    #[serde(default)]
    verified_name: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let mut args = env::args().skip(1);
    let mut token_arg = None;
    let mut phone_arg = None;
    let mut recipient_arg = None;
    let mut output_arg = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--token" => token_arg = args.next(),
            "--phone-id" => phone_arg = args.next(),
            "--recipient" => recipient_arg = args.next(),
            "--output" => output_arg = args.next(),
            _ => {}
        }
    }

    let token = token_arg
        .or_else(|| env::var("WHATSAPP_TOKEN").ok())
        .context("WHATSAPP_TOKEN missing (provide --token or export it)")?;

    let phone_id = phone_arg
        .or_else(|| env::var("WHATSAPP_PHONE_ID").ok())
        .context("WHATSAPP_PHONE_ID missing (provide --phone-id or export it)")?;

    let recipient = recipient_arg.or_else(|| env::var("WHATSAPP_RECIPIENT").ok());

    let client = reqwest::Client::new();
    let info = client
        .get(format!(
            "https://graph.facebook.com/v18.0/{phone_id}?fields=display_phone_number,verified_name"
        ))
        .bearer_auth(&token)
        .send()
        .await
        .context("failed to query phone info")?
        .error_for_status()
        .map_err(|err| anyhow!("phone info request failed: {err}"))?
        .json::<PhoneInfo>()
        .await
        .context("failed to decode phone info")?;

    println!(
        "Verified phone: {} ({})",
        info.display_phone_number
            .unwrap_or_else(|| "unknown".into()),
        info.verified_name.unwrap_or_else(|| "unverified".into())
    );

    if let Some(number) = recipient.as_ref() {
        println!("Test recipient configured: {number}");
    } else {
        println!("No WHATSAPP_RECIPIENT configured.");
    }

    if let Some(path) = output_arg.or_else(|| env::var("WHATSAPP_ENV_PATH").ok()) {
        persist_env(&path, &token, &phone_id, recipient.as_deref())?;
        println!(
            "Stored WHATSAPP_TOKEN/PHONE_ID{} in {}",
            if recipient.is_some() {
                "/RECIPIENT"
            } else {
                ""
            },
            path
        );
    } else {
        println!("Add to your .env:");
        println!("WHATSAPP_TOKEN={token}");
        println!("WHATSAPP_PHONE_ID={phone_id}");
        if let Some(number) = recipient {
            println!("WHATSAPP_RECIPIENT={number}");
        }
    }

    Ok(())
}

fn persist_env(path: &str, token: &str, phone_id: &str, recipient: Option<&str>) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open {path}"))?;

    writeln!(file, "WHATSAPP_TOKEN={token}")?;
    writeln!(file, "WHATSAPP_PHONE_ID={phone_id}")?;
    if let Some(number) = recipient {
        writeln!(file, "WHATSAPP_RECIPIENT={number}")?;
    }

    Ok(())
}
