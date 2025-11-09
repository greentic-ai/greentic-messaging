use crate::{MessageContext, TelemetryLabels, record_counter};
use url::Url;

const AUTH_CARD_RENDERED_COUNTER: &str = "auth_card_rendered";
const AUTH_CARD_CLICKED_COUNTER: &str = "auth_card_clicked";

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
    if let Some(url) = start_url {
        if let Some(domain) = start_url_domain(url) {
            enriched.extra.push(("start_url_domain".into(), domain));
        }
    }
    record_counter(AUTH_CARD_RENDERED_COUNTER, 1, &enriched);
}

fn start_url_domain(raw: &str) -> Option<String> {
    Url::parse(raw)
        .ok()
        .and_then(|parsed| parsed.host_str().map(|host| host.to_string()))
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
    record_counter(AUTH_CARD_CLICKED_COUNTER, 1, &labels);
}
