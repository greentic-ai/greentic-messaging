use crate::prelude::*;
use crate::Platform;
use std::hash::{Hash, Hasher};

#[derive(Clone, Debug, Eq)]
pub struct ProviderKey {
    pub platform: Platform,
    pub env: EnvId,
    pub tenant: TenantId,
    pub team: Option<TeamId>,
}

impl PartialEq for ProviderKey {
    fn eq(&self, other: &Self) -> bool {
        self.platform == other.platform
            && self.env == other.env
            && self.tenant == other.tenant
            && self.team == other.team
    }
}

impl Hash for ProviderKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.platform.hash(state);
        self.env.0.hash(state);
        self.tenant.0.hash(state);
        if let Some(team) = &self.team {
            team.0.hash(state);
        } else {
            "".hash(state);
        }
    }
}
