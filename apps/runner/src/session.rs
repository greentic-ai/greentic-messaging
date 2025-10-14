use gsm_core::MessageEnvelope;
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId {
    tenant: String,
    platform: String,
    chat_id: String,
    user_id: String,
    thread_id: Option<String>,
}

impl SessionId {
    pub fn from_env(env: &MessageEnvelope) -> Self {
        Self {
            tenant: env.tenant.clone(),
            platform: env.platform.as_str().to_string(),
            chat_id: env.chat_id.clone(),
            user_id: env.user_id.clone(),
            thread_id: env.thread_id.clone(),
        }
    }
}

#[derive(Clone, Default)]
pub struct Sessions(Arc<Mutex<HashMap<SessionId, Value>>>);

impl Sessions {
    pub fn get(&self, id: &SessionId) -> Value {
        self.0
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}))
    }
    pub fn put(&self, id: &SessionId, state: Value) {
        self.0.lock().unwrap().insert(id.clone(), state);
    }
}
