#[derive(Debug, Clone)]
pub struct TelemetryLabels {
    pub tenant: String,
    pub platform: Option<String>,
    pub chat_id: Option<String>,
    pub msg_id: Option<String>,
    pub extra: Vec<(String, String)>,
}

impl TelemetryLabels {
    pub fn new(tenant: impl Into<String>) -> Self {
        Self {
            tenant: tenant.into(),
            platform: None,
            chat_id: None,
            msg_id: None,
            extra: Vec::new(),
        }
    }

    pub fn tags(&self) -> Vec<(String, String)> {
        let mut tags = Vec::with_capacity(4 + self.extra.len());
        tags.push(("tenant".into(), self.tenant.clone()));
        if let Some(p) = &self.platform {
            tags.push(("platform".into(), p.clone()));
        }
        if let Some(chat) = &self.chat_id {
            tags.push(("chat_id".into(), chat.clone()));
        }
        if let Some(msg) = &self.msg_id {
            tags.push(("msg_id".into(), msg.clone()));
        }
        for (key, value) in &self.extra {
            tags.push((key.clone(), value.clone()));
        }
        tags
    }
}

#[derive(Debug, Clone)]
pub struct MessageContext {
    pub labels: TelemetryLabels,
}

impl MessageContext {
    pub fn new(labels: TelemetryLabels) -> Self {
        Self { labels }
    }
}
