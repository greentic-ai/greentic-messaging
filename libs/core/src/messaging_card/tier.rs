use serde::{Deserialize, Serialize};

/// Represents how expressive a rendered message card can be on a target platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    #[default]
    Basic,
    Advanced,
    Premium,
}

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Basic => "basic",
            Tier::Advanced => "advanced",
            Tier::Premium => "premium",
        }
    }

    pub fn clamp(self, target: Tier) -> Tier {
        self.min(target)
    }
}

/// Configuration describing how the engine should pick a tier for a platform.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TierPolicy {
    pub preferred: Tier,
    pub allow_downgrade: bool,
}

impl TierPolicy {
    pub fn new(preferred: Tier) -> Self {
        Self {
            preferred,
            allow_downgrade: true,
        }
    }

    pub fn resolve(&self, requested: Option<Tier>) -> Tier {
        match (requested, self.allow_downgrade) {
            (Some(requested), true) => requested.min(self.preferred),
            (Some(requested), false) => requested,
            (None, _) => self.preferred,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_policy_resolves_downgrades() {
        let policy = TierPolicy::new(Tier::Advanced);
        assert_eq!(policy.resolve(Some(Tier::Premium)), Tier::Advanced);
        assert_eq!(policy.resolve(Some(Tier::Basic)), Tier::Basic);
    }
}
