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

#[cfg(test)]
mod tests {
    use super::*;
    use gsm_core::{MessageEnvelope, Platform};

    fn sample_envelope() -> MessageEnvelope {
        MessageEnvelope {
            tenant: "acme".into(),
            platform: Platform::Slack,
            chat_id: "C1".into(),
            user_id: "U1".into(),
            thread_id: Some("T1".into()),
            msg_id: "msg-1".into(),
            text: Some("hello".into()),
            timestamp: "2024-01-01T00:00:00Z".into(),
            context: Default::default(),
        }
    }

    #[test]
    fn session_id_maps_envelope_fields() {
        let env = sample_envelope();
        let id = SessionId::from_env(&env);
        assert_eq!(id.tenant, "acme");
        assert_eq!(id.platform, "slack");
        assert_eq!(id.chat_id, "C1");
        assert_eq!(id.user_id, "U1");
        assert_eq!(id.thread_id.as_deref(), Some("T1"));
    }

    #[test]
    fn sessions_store_and_retrieve_state() {
        let env = sample_envelope();
        let id = SessionId::from_env(&env);

        let sessions = Sessions::default();
        assert_eq!(sessions.get(&id), serde_json::json!({})); // empty default
        sessions.put(&id, serde_json::json!({"seen": true}));
        assert_eq!(sessions.get(&id), serde_json::json!({"seen": true}));
    }
}
