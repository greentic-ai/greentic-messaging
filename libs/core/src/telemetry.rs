use anyhow::Result;
#[cfg(feature = "telemetry-client")]
use greentic_telemetry::client;
use greentic_telemetry::{self as telemetry, TelemetryConfig, TelemetryCtx};
use greentic_types::TenantCtx;
use tracing::Span;
use url::Url;

#[derive(Debug, Clone, Default)]
pub struct TelemetryHandle {
    _priv: (),
}

impl TelemetryHandle {
    pub fn noop() -> Self {
        Self { _priv: () }
    }
}

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

    pub fn tags(&self) -> Vec<(&str, String)> {
        let mut tags = Vec::with_capacity(4 + self.extra.len());
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
    pub fn new(labels: TelemetryLabels) -> Self {
        Self { labels }
    }
}

pub fn install(service_name: &str) -> Result<()> {
    telemetry::init_telemetry(TelemetryConfig {
        service_name: service_name.to_string(),
    })
}

pub fn set_current_tenant_ctx(ctx: TenantCtx) {
    let mut telemetry_ctx = TelemetryCtx::new(ctx.tenant.as_str().to_string());
    if let Some(session) = ctx.session_id() {
        telemetry_ctx = telemetry_ctx.with_session(session.to_string());
    }
    if let Some(flow) = ctx.flow_id.as_deref() {
        telemetry_ctx = telemetry_ctx.with_flow(flow.to_string());
    }
    if let Some(node) = ctx.node_id.as_deref() {
        telemetry_ctx = telemetry_ctx.with_node(node.to_string());
    }
    if let Some(provider) = ctx.provider_id.as_deref() {
        telemetry_ctx = telemetry_ctx.with_provider(provider.to_string());
    }

    telemetry::set_current_telemetry_ctx(telemetry_ctx);
}

pub fn telemetry_enabled() -> bool {
    true
}

pub fn with_common_fields(span: &Span, tenant: &str, chat_id: Option<&str>, msg_id: Option<&str>) {
    span.record("tenant", tracing::field::display(tenant));
    if let Some(chat_id) = chat_id {
        span.record("chat_id", tracing::field::display(chat_id));
    }
    if let Some(msg_id) = msg_id {
        span.record("msg_id", tracing::field::display(msg_id));
    }
}

pub fn record_counter(name: &'static str, value: u64, labels: &TelemetryLabels) {
    emit_metric(name, value as f64, labels);
}

pub fn record_histogram(name: &'static str, value: f64, labels: &TelemetryLabels) {
    emit_metric(name, value, labels);
}

pub fn record_gauge(name: &'static str, value: i64, labels: &TelemetryLabels) {
    emit_metric(name, value as f64, labels);
}
#[cfg(feature = "telemetry-client")]
fn emit_metric(name: &'static str, value: f64, labels: &TelemetryLabels) {
    let mut attrs: Vec<(String, String)> = Vec::new();
    attrs.push(("tenant".into(), labels.tenant.clone()));
    if let Some(platform) = labels.platform.as_deref() {
        attrs.push(("platform".into(), platform.to_string()));
    }
    if let Some(chat) = labels.chat_id.as_deref() {
        attrs.push(("chat_id".into(), chat.to_string()));
    }
    if let Some(msg) = labels.msg_id.as_deref() {
        attrs.push(("msg_id".into(), msg.to_string()));
    }
    for (key, value) in &labels.extra {
        attrs.push((key.clone(), value.clone()));
    }

    let refs: Vec<(&str, &str)> = attrs
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    client::metric(name, value, &refs);
}

#[cfg(not(feature = "telemetry-client"))]
fn emit_metric(_name: &'static str, _value: f64, _labels: &TelemetryLabels) {}

#[derive(Debug, Clone, Copy)]
pub enum AuthRenderMode {
    Pending,
    Native,
    Downgrade,
}

impl AuthRenderMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuthRenderMode::Pending => "pending",
            AuthRenderMode::Native => "native",
            AuthRenderMode::Downgrade => "downgrade",
        }
    }
}

pub fn record_auth_card_render(
    ctx: &MessageContext,
    provider: &str,
    mode: AuthRenderMode,
    connection_name: Option<&str>,
    start_url: Option<&str>,
    team: Option<&str>,
) {
    record_auth_card_render_with_labels(
        &ctx.labels,
        provider,
        mode,
        connection_name,
        start_url,
        team,
    );
}

pub fn record_auth_card_render_with_labels(
    labels: &TelemetryLabels,
    provider: &str,
    mode: AuthRenderMode,
    connection_name: Option<&str>,
    start_url: Option<&str>,
    team: Option<&str>,
) {
    let mut enriched = labels.clone();
    enriched
        .extra
        .push(("event".into(), "auth.card.rendered".into()));
    enriched
        .extra
        .push(("provider".into(), provider.to_string()));
    enriched
        .extra
        .push(("mode".into(), mode.as_str().to_string()));
    if let Some(team) = team {
        enriched.extra.push(("team".into(), team.to_string()));
    }
    if let Some(connection) = connection_name.filter(|value| !value.is_empty()) {
        enriched
            .extra
            .push(("connection_name".into(), connection.to_string()));
    }
    if let Some(url) = start_url
        .and_then(|raw| Url::parse(raw).ok())
        .and_then(|parsed| parsed.host_str().map(|host| host.to_string()))
    {
        enriched.extra.push(("start_url_domain".into(), url));
    }
    record_counter("auth_card_rendered", 1, &enriched);
}

pub fn record_auth_card_clicked(
    ctx: &MessageContext,
    provider: &str,
    platform: &str,
    message_id: Option<&str>,
    team: Option<&str>,
) {
    let mut labels = ctx.labels.clone();
    labels
        .extra
        .push(("event".into(), "auth.card.clicked".into()));
    labels.extra.push(("provider".into(), provider.to_string()));
    labels.platform = Some(platform.to_string());
    if let Some(message_id) = message_id {
        labels
            .extra
            .push(("message_id".into(), message_id.to_string()));
    }
    if let Some(team) = team {
        labels.extra.push(("team".into(), team.to_string()));
    }
    record_counter("auth_card_clicked", 1, &labels);
}
