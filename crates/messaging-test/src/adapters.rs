use std::env;

use gsm_core::Platform;

pub struct AdapterConfig {
    pub mode: AdapterMode,
}

impl AdapterConfig {
    pub fn load(force_dry_run: bool) -> Self {
        Self {
            mode: if force_dry_run {
                AdapterMode::DryRun
            } else {
                AdapterMode::Real
            },
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum AdapterMode {
    Real,
    DryRun,
}

pub struct AdapterTarget {
    pub name: &'static str,
    pub platform: Platform,
    pub enabled: bool,
    pub reason: Option<String>,
    pub mode: AdapterMode,
}

impl AdapterTarget {
    fn new(
        name: &'static str,
        platform: Platform,
        enabled: bool,
        reason: Option<String>,
        mode: AdapterMode,
    ) -> Self {
        Self {
            name,
            platform,
            enabled,
            reason,
            mode,
        }
    }
}

const ADAPTERS: &[(&str, Platform, &[&str])] = &[
    ("teams", Platform::Teams, &["MS_GRAPH_TOKEN"]),
    ("webex", Platform::Webex, &["WEBEX_BOT_TOKEN"]),
    ("slack", Platform::Slack, &["SLACK_BOT_TOKEN"]),
    ("webchat", Platform::WebChat, &["WEBCHAT_SECRET"]),
    ("telegram", Platform::Telegram, &["TELEGRAM_BOT_TOKEN"]),
    ("whatsapp", Platform::WhatsApp, &["WHATSAPP_TOKEN"]),
];

pub fn registry_from_env(mode: AdapterMode) -> Vec<AdapterTarget> {
    ADAPTERS
        .iter()
        .map(|(name, platform, reqs)| {
            let missing: Vec<_> = reqs
                .iter()
                .filter(|key| env::var(key).is_err())
                .copied()
                .collect();
            let enabled = matches!(mode, AdapterMode::DryRun) || missing.is_empty();
            let reason = if missing.is_empty() {
                None
            } else {
                Some(format!("missing envs: {}", missing.join(", ")))
            };
            AdapterTarget::new(name, platform.clone(), enabled, reason, mode)
        })
        .collect()
}
