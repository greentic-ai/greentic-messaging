use super::{models::*, traits::AdminError};

const MAX_EXTRA_PARAM_VALUE_LEN: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportMode {
    Plan,
    Ensure,
}

impl ReportMode {
    fn as_str(self) -> &'static str {
        match self {
            ReportMode::Plan => "plan",
            ReportMode::Ensure => "ensure",
        }
    }
}

pub struct ProvisionReportBuilder {
    report: ProvisionReport,
}

impl ProvisionReportBuilder {
    pub fn new(provider: &str, tenant: Option<&str>, mode: ReportMode) -> Self {
        ProvisionReportBuilder {
            report: ProvisionReport {
                provider: provider.to_string(),
                tenant: tenant.map(|t| t.to_string()),
                created: Vec::new(),
                updated: Vec::new(),
                skipped: Vec::new(),
                warnings: Vec::new(),
                secret_keys_written: Vec::new(),
                mode: Some(mode.as_str().to_string()),
            },
        }
    }

    pub fn created(&mut self, value: impl Into<String>) {
        self.report.created.push(value.into());
    }

    pub fn updated(&mut self, value: impl Into<String>) {
        self.report.updated.push(value.into());
    }

    pub fn skipped(&mut self, value: impl Into<String>) {
        self.report.skipped.push(value.into());
    }

    pub fn warn(&mut self, value: impl Into<String>) {
        self.report.warnings.push(value.into());
    }

    pub fn secret_written(&mut self, key: impl Into<String>) {
        self.report.secret_keys_written.push(key.into());
    }

    pub fn finish(self) -> ProvisionReport {
        self.report
    }
}

pub fn validate_global_app(app: &DesiredGlobalApp) -> Result<(), AdminError> {
    ensure_non_empty("display_name", &app.display_name)?;
    ensure_no_control_chars("display_name", &app.display_name)?;
    if let Some(extra) = &app.extra_params {
        validate_extra_params(extra)?;
    }
    Ok(())
}

pub fn validate_resource(resource: &ResourceSpec) -> Result<(), AdminError> {
    ensure_non_empty("resource.id", &resource.id)?;
    ensure_no_control_chars("resource.id", &resource.id)?;
    if let Some(name) = &resource.display_name {
        ensure_no_control_chars("resource.display_name", name)?;
    }
    Ok(())
}

pub fn validate_tenant_binding(binding: &DesiredTenantBinding) -> Result<(), AdminError> {
    ensure_non_empty("tenant_key", &binding.tenant_key)?;
    ensure_no_control_chars("tenant_key", &binding.tenant_key)?;
    ensure_non_empty("provider_tenant_id", &binding.provider_tenant_id)?;
    ensure_no_control_chars("provider_tenant_id", &binding.provider_tenant_id)?;
    if let Some(extra) = &binding.extra_params {
        validate_extra_params(extra)?;
    }
    for resource in &binding.resources {
        validate_resource(resource)?;
    }
    Ok(())
}

fn ensure_non_empty(field: &str, value: &str) -> Result<(), AdminError> {
    if value.trim().is_empty() {
        return Err(AdminError::Validation(format!("{field} must not be empty")));
    }
    Ok(())
}

fn ensure_no_control_chars(field: &str, value: &str) -> Result<(), AdminError> {
    if value.chars().any(is_disallowed_control) {
        return Err(AdminError::Validation(format!(
            "{field} contains control characters"
        )));
    }
    Ok(())
}

fn validate_extra_params(
    params: &std::collections::BTreeMap<String, String>,
) -> Result<(), AdminError> {
    for (key, value) in params {
        if value.len() > MAX_EXTRA_PARAM_VALUE_LEN {
            return Err(AdminError::Validation(format!(
                "extra_params value for {key} exceeds {MAX_EXTRA_PARAM_VALUE_LEN} chars"
            )));
        }
        if value.chars().any(is_disallowed_control) {
            return Err(AdminError::Validation(format!(
                "extra_params value for {key} contains control characters"
            )));
        }
    }
    Ok(())
}

fn is_disallowed_control(ch: char) -> bool {
    (ch as u32) < 0x20 && ch != '\n' && ch != '\r'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_control_chars() {
        assert!(is_disallowed_control('\u{0001}'));
        assert!(!is_disallowed_control('\n'));
    }
}
