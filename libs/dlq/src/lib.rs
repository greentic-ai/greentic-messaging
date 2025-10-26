//! Helpers for emitting failed messages to dedicated delay/dead-letter subjects.
//!
//! ```no_run
//! use gsm_dlq::{DlqError, DlqPublisher};
//!
//! # fn main() -> anyhow::Result<()> {
//! # let rt = tokio::runtime::Runtime::new()?;
//! rt.block_on(async {
//!     let client = async_nats::connect("nats://127.0.0.1:4222").await?;
//!     let dlq = DlqPublisher::new("egress", client).await?;
//!     dlq
//!         .publish(
//!             "acme",
//!             "webex",
//!             "msg-1",
//!             1,
//!             DlqError {
//!                 code: "E_SEND".into(),
//!                 message: "provider returned 500".into(),
//!                 stage: Some("egress".into()),
//!             },
//!             &serde_json::json!({"chat_id": "room-1"}),
//!         )
//!         .await?;
//!     anyhow::Ok(())
//! })
//! # }
//! ```

use std::collections::HashMap;

use anyhow::{Context, Result};
use async_nats::{
    jetstream::{
        consumer::{pull::Config as PullConfig, AckPolicy, DeliverPolicy},
        stream::{Config as StreamConfig, RetentionPolicy},
        Context as JsContext,
    },
    Client,
};
use futures::TryStreamExt;
use gsm_telemetry::{record_counter, TelemetryLabels};
use nanoid::nanoid;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use tracing::{info, warn};

const DLQ_ENABLED_ENV: &str = "DLQ_ENABLED";
const DLQ_SUBJECT_FMT_ENV: &str = "DLQ_SUBJECT_FMT";
const REPLAY_SUBJECT_FMT_ENV: &str = "REPLAY_SUBJECT_FMT";
const DEFAULT_DLQ_SUBJECT_FMT: &str = "dlq.{tenant}.{stage}";
const DEFAULT_REPLAY_SUBJECT_FMT: &str = "replay.{tenant}.{stage}";
const DLQ_STREAM_NAME: &str = "DLQ";

/// Error metadata stored alongside each DLQ entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DlqError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub stage: Option<String>,
}

/// Payload stored for each DLQ message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DlqRecord {
    pub tenant: String,
    pub stage: String,
    pub platform: String,
    pub msg_id: String,
    pub retries: u32,
    pub ts: String,
    pub error: DlqError,
    pub envelope: Value,
}

#[derive(Clone)]
pub struct DlqPublisher {
    #[allow(dead_code)]
    client: Client,
    js: JsContext,
    stage: String,
    subject_fmt: String,
    enabled: bool,
}

impl DlqPublisher {
    pub async fn new(stage: &str, client: Client) -> Result<Self> {
        let enabled = std::env::var(DLQ_ENABLED_ENV)
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(true);
        let fmt =
            std::env::var(DLQ_SUBJECT_FMT_ENV).unwrap_or_else(|_| DEFAULT_DLQ_SUBJECT_FMT.into());

        let js = async_nats::jetstream::new(client.clone());
        ensure_stream(&js, &fmt).await?;

        Ok(Self {
            client,
            js,
            stage: stage.to_string(),
            subject_fmt: fmt,
            enabled,
        })
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub async fn publish<S: Serialize>(
        &self,
        tenant: &str,
        platform: &str,
        msg_id: &str,
        retries: u32,
        error: DlqError,
        envelope: &S,
    ) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }

        let subject = format_subject(&self.subject_fmt, tenant, &self.stage, Some(platform));
        let ts = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into());
        let record = DlqRecord {
            tenant: tenant.to_string(),
            stage: self.stage.clone(),
            platform: platform.to_string(),
            msg_id: msg_id.to_string(),
            retries,
            ts,
            error: DlqError {
                stage: Some(self.stage.clone()),
                ..error
            },
            envelope: serde_json::to_value(envelope)?,
        };

        let payload = serde_json::to_vec(&record)?;
        self.js
            .publish(subject.clone(), payload.into())
            .await
            .with_context(|| format!("publish DLQ entry to {subject}"))?;

        let mut labels = TelemetryLabels {
            tenant: tenant.to_string(),
            platform: None,
            chat_id: None,
            msg_id: None,
            extra: Vec::new(),
        };
        labels.extra.push(("stage".into(), self.stage.clone()));
        labels
            .extra
            .push(("code".into(), record.error.code.clone()));
        record_counter("dlq_published", 1, &labels);
        info!(
            tenant = %record.tenant,
            stage = %record.stage,
            platform = %record.platform,
            msg_id = %record.msg_id,
            code = %record.error.code,
            "dlq entry published"
        );
        Ok(())
    }
}

async fn ensure_stream(js: &JsContext, subject_fmt: &str) -> Result<()> {
    let pattern = subject_fmt
        .replace("{tenant}", "*")
        .replace("{stage}", "*")
        .replace("{platform}", "*");
    let cfg = StreamConfig {
        name: DLQ_STREAM_NAME.into(),
        subjects: vec![pattern],
        retention: RetentionPolicy::WorkQueue,
        max_messages_per_subject: -1,
        max_messages: -1,
        max_bytes: -1,
        description: Some("Greentic DLQ".into()),
        ..StreamConfig::default()
    };

    match js.get_stream(DLQ_STREAM_NAME).await {
        Ok(_) => Ok(()),
        Err(_) => {
            js.create_stream(cfg).await.context("create DLQ stream")?;
            Ok(())
        }
    }
}

pub fn replay_subject(tenant: &str, stage: &str) -> String {
    let fmt =
        std::env::var(REPLAY_SUBJECT_FMT_ENV).unwrap_or_else(|_| DEFAULT_REPLAY_SUBJECT_FMT.into());
    format_subject(&fmt, tenant, stage, None)
}

pub fn format_subject(fmt: &str, tenant: &str, stage: &str, platform: Option<&str>) -> String {
    let mut map = HashMap::new();
    map.insert("tenant", tenant);
    map.insert("stage", stage);
    if let Some(p) = platform {
        map.insert("platform", p);
    }
    let mut out = String::new();
    let mut chars = fmt.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '{' {
            let mut key = String::new();
            while let Some(&next) = chars.peek() {
                chars.next();
                if next == '}' {
                    break;
                }
                key.push(next);
            }
            if let Some(val) = map.get(key.as_str()) {
                out.push_str(val);
            } else {
                out.push_str("");
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Representation returned by DLQ consumers and CLI.
#[derive(Debug, Clone)]
pub struct DlqEntry {
    pub record: DlqRecord,
    pub sequence: u64,
}

pub async fn list_entries(
    client: &Client,
    tenant: &str,
    stage: &str,
    limit: usize,
) -> Result<Vec<DlqEntry>> {
    let js = async_nats::jetstream::new(client.clone());
    ensure_stream(
        &js,
        &std::env::var(DLQ_SUBJECT_FMT_ENV).unwrap_or_else(|_| DEFAULT_DLQ_SUBJECT_FMT.into()),
    )
    .await?;
    let stream = js.get_stream(DLQ_STREAM_NAME).await?;
    let filter_subject = format_subject(
        &std::env::var(DLQ_SUBJECT_FMT_ENV).unwrap_or_else(|_| DEFAULT_DLQ_SUBJECT_FMT.into()),
        tenant,
        stage,
        None,
    );
    let durable = format!("dlq-list-{tenant}-{stage}-{rand}", rand = nanoid!(6));
    let consumer = stream
        .create_consumer(PullConfig {
            durable_name: Some(durable.clone()),
            deliver_policy: DeliverPolicy::All,
            ack_policy: AckPolicy::None,
            filter_subject,
            ..Default::default()
        })
        .await?;
    let mut messages = consumer.fetch().max_messages(limit).messages().await?;
    let mut out = Vec::new();
    while let Some(msg) = messages.try_next().await.map_err(|e| anyhow::anyhow!(e))? {
        if let Ok(record) = serde_json::from_slice::<DlqRecord>(&msg.payload) {
            out.push(DlqEntry {
                sequence: msg.info().map(|info| info.stream_sequence).unwrap_or(0),
                record,
            });
        }
    }
    Ok(out)
}

pub async fn get_entry(client: &Client, sequence: u64) -> Result<Option<DlqEntry>> {
    let js = async_nats::jetstream::new(client.clone());
    match js.get_stream(DLQ_STREAM_NAME).await {
        Ok(stream) => match stream.direct_get(sequence).await {
            Ok(message) => {
                if let Ok(record) = serde_json::from_slice::<DlqRecord>(&message.payload) {
                    Ok(Some(DlqEntry { sequence, record }))
                } else {
                    Ok(None)
                }
            }
            Err(err) => {
                warn!("failed to fetch dlq message: {err}");
                Ok(None)
            }
        },
        Err(_) => Ok(None),
    }
}

pub async fn replay_entry(client: &Client, entry: &DlqEntry, target_stage: &str) -> Result<()> {
    let subject = format_subject(
        &std::env::var(REPLAY_SUBJECT_FMT_ENV)
            .unwrap_or_else(|_| DEFAULT_REPLAY_SUBJECT_FMT.into()),
        &entry.record.tenant,
        target_stage,
        None,
    );
    client
        .publish(
            subject.clone(),
            serde_json::to_vec(&entry.record.envelope)?.into(),
        )
        .await
        .with_context(|| format!("replay publish to {subject}"))?;
    Ok(())
}

pub async fn replay_entries(
    client: &Client,
    tenant: &str,
    stage: &str,
    target_stage: &str,
    limit: usize,
) -> Result<Vec<DlqEntry>> {
    let js = async_nats::jetstream::new(client.clone());
    ensure_stream(
        &js,
        &std::env::var(DLQ_SUBJECT_FMT_ENV).unwrap_or_else(|_| DEFAULT_DLQ_SUBJECT_FMT.into()),
    )
    .await?;
    let stream = js.get_stream(DLQ_STREAM_NAME).await?;
    let filter_subject = format_subject(
        &std::env::var(DLQ_SUBJECT_FMT_ENV).unwrap_or_else(|_| DEFAULT_DLQ_SUBJECT_FMT.into()),
        tenant,
        stage,
        None,
    );
    let durable = format!("dlq-replay-{tenant}-{stage}-{rand}", rand = nanoid!(6));
    let consumer = stream
        .create_consumer(PullConfig {
            durable_name: Some(durable.clone()),
            deliver_policy: DeliverPolicy::All,
            ack_policy: AckPolicy::Explicit,
            filter_subject,
            ..Default::default()
        })
        .await?;
    let mut messages = consumer.fetch().max_messages(limit).messages().await?;
    let mut processed = Vec::new();
    while let Some(msg) = messages.try_next().await.map_err(|e| anyhow::anyhow!(e))? {
        let sequence = msg.info().map(|info| info.stream_sequence).unwrap_or(0);
        match serde_json::from_slice::<DlqRecord>(&msg.payload) {
            Ok(record) => {
                let entry = DlqEntry { record, sequence };
                replay_entry(client, &entry, target_stage).await?;
                msg.ack().await.map_err(|e| anyhow::anyhow!(e))?;
                processed.push(entry);
            }
            Err(err) => {
                warn!("failed to parse dlq record: {err}");
                msg.ack().await.map_err(|e| anyhow::anyhow!(e))?;
            }
        }
    }
    Ok(processed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn format_subject_inserts_placeholders() {
        let s = format_subject(
            "dlq.{tenant}.{stage}.{platform}",
            "t1",
            "egress",
            Some("slack"),
        );
        assert_eq!(s, "dlq.t1.egress.slack");
    }

    #[test]
    fn format_subject_handles_missing_platform() {
        let s = format_subject("dlq.{tenant}.{stage}.{platform}", "t1", "egress", None);
        assert_eq!(s, "dlq.t1.egress.");
    }

    #[test]
    fn replay_subject_uses_env_override() {
        let _guard = env_lock().lock().unwrap();
        std::env::set_var(REPLAY_SUBJECT_FMT_ENV, "replay.{tenant}.{stage}");
        assert_eq!(replay_subject("acme", "translate"), "replay.acme.translate");
        std::env::remove_var(REPLAY_SUBJECT_FMT_ENV);
    }

    #[test]
    fn subject_formatting_works() {
        let r = format_subject("replay.{tenant}.{stage}", "t1", "translate", None);
        assert_eq!(r, "replay.t1.translate");
    }

    #[test]
    fn record_roundtrips_json() {
        let record = DlqRecord {
            tenant: "t1".into(),
            stage: "egress".into(),
            platform: "slack".into(),
            msg_id: "abc".into(),
            retries: 2,
            ts: "2024-01-01T00:00:00Z".into(),
            error: DlqError {
                code: "E_SEND".into(),
                message: "429".into(),
                stage: Some("egress".into()),
            },
            envelope: serde_json::json!({"hello": "world"}),
        };
        let serialized = serde_json::to_string(&record).expect("serialize");
        let parsed: DlqRecord = serde_json::from_str(&serialized).expect("parse");
        assert_eq!(parsed.msg_id, "abc");
        assert_eq!(parsed.error.code, "E_SEND");
        assert_eq!(parsed.envelope["hello"], "world");
    }
}
