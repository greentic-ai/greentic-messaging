#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteContext {
    env: String,
    tenant: String,
    team: Option<String>,
}

impl RouteContext {
    pub fn new(env: String, tenant: String, team: Option<String>) -> Self {
        Self { env, tenant, team }
    }

    pub fn env(&self) -> &str {
        &self.env
    }

    pub fn tenant(&self) -> &str {
        &self.tenant
    }

    pub fn team(&self) -> Option<&str> {
        self.team.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_context_holds_values() {
        let ctx = RouteContext::new("dev".into(), "acme".into(), Some("support".into()));
        assert_eq!(ctx.env(), "dev");
        assert_eq!(ctx.tenant(), "acme");
        assert_eq!(ctx.team(), Some("support"));
    }
}
