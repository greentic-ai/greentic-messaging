use std::{sync::Arc, time::Duration as StdDuration};

use anyhow::{Context, Result};
use async_nats::jetstream::{
    self, Context as JsContext,
    context::KeyValueErrorKind,
    kv::{self, CreateErrorKind},
};
use async_trait::async_trait;
use serde::Serialize;

#[async_trait]
pub trait NonceStore: Send + Sync {
    async fn consume(&self, tenant: &str, jti: &str, nonce: &str, ttl_secs: u64) -> Result<bool>;
}

#[derive(Clone)]
pub struct JetstreamNonceStore {
    bucket: kv::Store,
}

impl JetstreamNonceStore {
    pub async fn new(js: &JsContext, namespace: &str) -> Result<Self> {
        let bucket = match js.get_key_value(namespace).await {
            Ok(store) => store,
            Err(err) if err.kind() == KeyValueErrorKind::GetBucket => js
                .create_key_value(kv::Config {
                    bucket: namespace.to_string(),
                    history: 1,
                    max_age: StdDuration::from_secs(0),
                    ..Default::default()
                })
                .await
                .context("create security nonce bucket")?,
            Err(err) => return Err(err.into()),
        };
        Ok(Self { bucket })
    }

    fn key(&self, tenant: &str, jti: &str) -> String {
        format!("nonce/{tenant}/{jti}")
    }
}

#[async_trait]
impl NonceStore for JetstreamNonceStore {
    async fn consume(&self, tenant: &str, jti: &str, nonce: &str, ttl_secs: u64) -> Result<bool> {
        let key = self.key(tenant, jti);
        let payload = serde_json::to_vec(&NonceRecord { nonce })?;
        let ttl = StdDuration::from_secs(ttl_secs.max(1));
        match self.bucket.create_with_ttl(key, payload.into(), ttl).await {
            Ok(_) => Ok(true),
            Err(err) if err.kind() == CreateErrorKind::AlreadyExists => Ok(false),
            Err(err) => Err(anyhow::anyhow!(err).context("store nonce")),
        }
    }
}

#[derive(Serialize)]
struct NonceRecord<'a> {
    nonce: &'a str,
}

pub type SharedNonceStore = Arc<dyn NonceStore>;

const DEFAULT_NONCE_NAMESPACE: &str = "security";

pub async fn default_nonce_store(client: &async_nats::Client) -> Result<JetstreamNonceStore> {
    let js = jetstream::new(client.clone());
    JetstreamNonceStore::new(&js, DEFAULT_NONCE_NAMESPACE).await
}
