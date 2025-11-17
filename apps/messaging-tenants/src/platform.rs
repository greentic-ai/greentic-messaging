use clap::ValueEnum;
use gsm_core::Platform;

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum PlatformArg {
    Slack,
    Teams,
    Telegram,
    WhatsApp,
    WebChat,
    Webex,
}

impl PlatformArg {
    pub fn as_str(self) -> &'static str {
        match self {
            PlatformArg::Slack => "slack",
            PlatformArg::Teams => "teams",
            PlatformArg::Telegram => "telegram",
            PlatformArg::WhatsApp => "whatsapp",
            PlatformArg::WebChat => "webchat",
            PlatformArg::Webex => "webex",
        }
    }
}

impl From<PlatformArg> for Platform {
    fn from(value: PlatformArg) -> Self {
        match value {
            PlatformArg::Slack => Platform::Slack,
            PlatformArg::Teams => Platform::Teams,
            PlatformArg::Telegram => Platform::Telegram,
            PlatformArg::WhatsApp => Platform::WhatsApp,
            PlatformArg::WebChat => Platform::WebChat,
            PlatformArg::Webex => Platform::Webex,
        }
    }
}
