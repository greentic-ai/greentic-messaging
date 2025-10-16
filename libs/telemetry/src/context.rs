use gsm_core::{MessageEnvelope, OutMessage};

#[derive(Debug, Clone)]
pub struct TelemetryLabels {
    pub tenant: String,
    pub platform: Option<String>,
    pub chat_id: Option<String>,
    pub msg_id: Option<String>,
    pub extra: Vec<(String, String)>,
}

impl TelemetryLabels {
    pub fn from_out(out: &OutMessage) -> Self {
        Self {
            tenant: out.tenant.clone(),
            platform: Some(out.platform.as_str().to_string()),
            chat_id: Some(out.chat_id.clone()),
            msg_id: Some(out.message_id()),
            extra: Vec::new(),
        }
    }

    pub fn from_envelope(env: &MessageEnvelope) -> Self {
        Self {
            tenant: env.tenant.clone(),
            platform: Some(env.platform.as_str().to_string()),
            chat_id: Some(env.chat_id.clone()),
            msg_id: Some(env.msg_id.clone()),
            extra: Vec::new(),
        }
    }

    pub fn tags(&self) -> Vec<(&str, String)> {
        let mut tags = Vec::with_capacity(4);
        tags.push(("tenant", self.tenant.clone()));
        if let Some(p) = &self.platform {
            tags.push(("platform", p.clone()));
        }
        if let Some(chat) = &self.chat_id {
            tags.push(("chat_id", chat.clone()));
        }
        if let Some(msg) = &self.msg_id {
            tags.push(("msg_id", msg.clone()));
        }
        for (key, value) in &self.extra {
            tags.push((key.as_str(), value.clone()));
        }
        tags
    }
}

#[derive(Debug, Clone)]
pub struct MessageContext {
    pub labels: TelemetryLabels,
}

impl MessageContext {
    pub fn from_out(out: &OutMessage) -> Self {
        Self {
            labels: TelemetryLabels::from_out(out),
        }
    }

    pub fn from_envelope(env: &MessageEnvelope) -> Self {
        Self {
            labels: TelemetryLabels::from_envelope(env),
        }
    }
}
