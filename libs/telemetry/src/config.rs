use std::env;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelemetryProtocol {
    Grpc,
    HttpProtobuf,
}

#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    pub endpoint: String,
    pub protocol: TelemetryProtocol,
    pub service_name: String,
    pub service_version: String,
    pub environment: String,
    pub json_logs: bool,
    pub enabled: bool,
}

impl TelemetryConfig {
    pub fn from_env(default_service_name: &str, default_service_version: &str) -> Self {
        let endpoint = env::var("OTEL_EXPORTER_OTLP_ENDPOINT").unwrap_or_default();
        let protocol = env::var("OTEL_EXPORTER_OTLP_PROTOCOL")
            .map(|v| match v.to_lowercase().as_str() {
                "http" | "http/protobuf" => TelemetryProtocol::HttpProtobuf,
                _ => TelemetryProtocol::Grpc,
            })
            .unwrap_or(TelemetryProtocol::Grpc);
        let service_name =
            env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| default_service_name.to_string());
        let service_version = env::var("OTEL_SERVICE_VERSION")
            .unwrap_or_else(|_| default_service_version.to_string());
        let environment = env::var("OTEL_RESOURCE_ATTRIBUTES")
            .ok()
            .and_then(parse_environment_from_resource)
            .unwrap_or_else(|| env::var("DEPLOYMENT_ENV").unwrap_or_else(|_| "dev".into()));
        let json_logs = env::var("LOG_FORMAT")
            .map(|v| !matches!(v.to_lowercase().as_str(), "text" | "pretty" | "plain"))
            .unwrap_or(true);
        let enabled_flag = env::var("ENABLE_OTEL")
            .map(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);
        let enabled = enabled_flag && !endpoint.trim().is_empty();

        Self {
            endpoint,
            protocol,
            service_name,
            service_version,
            environment,
            json_logs,
            enabled,
        }
    }

    pub fn exporter_enabled(&self) -> bool {
        self.enabled && !self.endpoint.trim().is_empty()
    }
}

fn parse_environment_from_resource(value: String) -> Option<String> {
    for kv in value.split(',') {
        let mut parts = kv.splitn(2, '=');
        let key = parts.next()?.trim();
        let val = parts.next()?.trim();
        if key == "deployment.environment" {
            return Some(val.to_string());
        }
    }
    None
}
