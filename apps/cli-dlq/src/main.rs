use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use gsm_dlq::{get_entry, list_entries, replay_entries, DlqEntry};
use serde::Serialize;

#[derive(Parser, Debug)]
#[command(author, version, about = "Greentic Messaging DLQ CLI")]
struct Cli {
    /// Emit JSON output
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// List DLQ entries for a tenant/stage
    List {
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        stage: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Show a DLQ entry by stream sequence id
    Show {
        #[arg()]
        sequence: u64,
    },
    /// Replay DLQ entries to another stage
    Replay {
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        stage: String,
        #[arg(long)]
        to: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
}

#[derive(Serialize)]
struct ListEntry {
    sequence: u64,
    tenant: String,
    stage: String,
    platform: String,
    msg_id: String,
    code: String,
    retries: u32,
    ts: String,
}

#[derive(Serialize)]
struct ShowEntry<'a> {
    sequence: u64,
    record: &'a gsm_dlq::DlqRecord,
}

#[derive(Serialize)]
struct ReplayResult {
    target_stage: String,
    processed: Vec<ListEntry>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let client = async_nats::connect(nats_url).await?;

    match cli.command {
        Commands::List {
            tenant,
            stage,
            limit,
        } => {
            let entries = list_entries(&client, &tenant, &stage, limit).await?;
            if cli.json {
                let payload: Vec<_> = entries.iter().map(list_entry).collect();
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else if entries.is_empty() {
                println!("No DLQ entries for tenant={tenant} stage={stage}");
            } else {
                print_table(&entries);
            }
        }
        Commands::Show { sequence } => {
            let Some(entry) = get_entry(&client, sequence).await? else {
                bail!("dlq entry {sequence} not found");
            };
            if cli.json {
                let payload = ShowEntry {
                    sequence,
                    record: &entry.record,
                };
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                println!("sequence: {}", sequence);
                println!("tenant  : {}", entry.record.tenant);
                println!("stage   : {}", entry.record.stage);
                println!("platform: {}", entry.record.platform);
                println!("msg_id  : {}", entry.record.msg_id);
                println!("code    : {}", entry.record.error.code);
                println!("message : {}", entry.record.error.message);
                println!("retries : {}", entry.record.retries);
                println!("timestamp: {}", entry.record.ts);
                println!(
                    "envelope: {}",
                    serde_json::to_string_pretty(&entry.record.envelope)?
                );
            }
        }
        Commands::Replay {
            tenant,
            stage,
            to,
            limit,
        } => {
            let processed = replay_entries(&client, &tenant, &stage, &to, limit).await?;
            if cli.json {
                let payload = ReplayResult {
                    target_stage: to.clone(),
                    processed: processed.iter().map(list_entry).collect(),
                };
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else if processed.is_empty() {
                println!("No DLQ entries replayed for tenant={tenant} stage={stage}");
            } else {
                println!("Replayed {} entries to stage {to}", processed.len());
                print_table(&processed);
            }
        }
    }

    Ok(())
}

fn list_entry(entry: &DlqEntry) -> ListEntry {
    ListEntry {
        sequence: entry.sequence,
        tenant: entry.record.tenant.clone(),
        stage: entry.record.stage.clone(),
        platform: entry.record.platform.clone(),
        msg_id: entry.record.msg_id.clone(),
        code: entry.record.error.code.clone(),
        retries: entry.record.retries,
        ts: entry.record.ts.clone(),
    }
}

fn print_table(entries: &[DlqEntry]) {
    println!(
        "{:<8} {:<8} {:<8} {:<10} {:<8} {:<7} {:<}",
        "SEQ", "TENANT", "STAGE", "PLATFORM", "CODE", "RETRY", "TS"
    );
    for entry in entries {
        println!(
            "{:<8} {:<8} {:<8} {:<10} {:<8} {:<7} {}",
            entry.sequence,
            entry.record.tenant,
            entry.record.stage,
            entry.record.platform,
            entry.record.error.code,
            entry.record.retries,
            entry.record.ts
        );
    }
}
