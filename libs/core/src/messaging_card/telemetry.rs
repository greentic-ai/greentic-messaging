use crate::messaging_card::tier::Tier;

#[derive(Debug, Clone, PartialEq)]
pub enum TelemetryEvent {
    Rendered {
        platform: String,
        tier: Tier,
        warnings: usize,
        used_modal: bool,
    },
    Downgraded {
        from: Tier,
        to: Tier,
    },
}

pub trait TelemetryHook: Send + Sync {
    fn emit(&self, event: TelemetryEvent);
}

#[derive(Default)]
pub struct NullTelemetry;

impl TelemetryHook for NullTelemetry {
    fn emit(&self, _event: TelemetryEvent) {}
}

pub struct CardTelemetry<'a> {
    hook: &'a dyn TelemetryHook,
}

impl<'a> CardTelemetry<'a> {
    pub fn new(hook: &'a dyn TelemetryHook) -> Self {
        Self { hook }
    }

    pub fn downgrading(&self, from: Tier, to: Tier) {
        self.hook.emit(TelemetryEvent::Downgraded { from, to });
    }

    pub fn rendered(&self, platform: &str, tier: Tier, warnings: usize, used_modal: bool) {
        self.hook.emit(TelemetryEvent::Rendered {
            platform: platform.to_string(),
            tier,
            warnings,
            used_modal,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestTelemetry {
        pub events: std::sync::Mutex<Vec<TelemetryEvent>>,
    }

    impl TestTelemetry {
        fn new() -> Self {
            Self {
                events: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    impl TelemetryHook for TestTelemetry {
        fn emit(&self, event: TelemetryEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    #[test]
    fn telemetry_records_events() {
        let hook = TestTelemetry::new();
        let telemetry = CardTelemetry::new(&hook);
        telemetry.downgrading(Tier::Premium, Tier::Basic);
        telemetry.rendered("teams", Tier::Basic, 1, true);
        let events = hook.events.lock().unwrap();
        assert_eq!(events.len(), 2);
    }
}
