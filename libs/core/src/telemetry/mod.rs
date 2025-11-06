#[derive(Debug, Clone, Default)]
pub struct TelemetryHandle {
    _priv: (),
}

impl TelemetryHandle {
    pub fn noop() -> Self {
        Self { _priv: () }
    }
}
