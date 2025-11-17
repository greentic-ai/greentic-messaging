use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use url::Url;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DesiredGlobalApp {
    pub display_name: String,
    pub redirect_uris: Vec<Url>,
    pub allowed_scopes: Vec<String>,
    pub capabilities: Vec<String>,
    pub extra_params: Option<BTreeMap<String, String>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum CredentialPolicy {
    ClientSecret { rotate_days: u32 },
    Certificate { subject: String, validity_days: u32 },
    None,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ResourceSpec {
    pub kind: String,
    pub id: String,
    pub display_name: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DesiredTenantBinding {
    pub tenant_key: String,
    pub provider_tenant_id: String,
    pub requested_scopes: Vec<String>,
    pub resources: Vec<ResourceSpec>,
    pub credential_policy: CredentialPolicy,
    pub extra_params: Option<BTreeMap<String, String>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ProvisionCaps {
    pub model: String,
    pub requires_global_bootstrap: bool,
    pub supports_rsc: bool,
    pub supports_webhooks: bool,
    pub supports_per_resource_install: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ProvisionReport {
    pub provider: String,
    pub tenant: Option<String>,
    pub created: Vec<String>,
    pub updated: Vec<String>,
    pub skipped: Vec<String>,
    pub warnings: Vec<String>,
    pub secret_keys_written: Vec<String>,
    pub mode: Option<String>,
}
