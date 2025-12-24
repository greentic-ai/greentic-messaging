#![cfg(all(feature = "providers-bundle-tests", providers_bundle_present))]
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use gsm_core::{RenderPlan, RenderTier, RenderWarning};
use serde_json::{Value, json};
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine, Store};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderBackend {
    Legacy,
    New,
}

pub fn backend() -> ProviderBackend {
    match std::env::var("GREENTIC_TEST_PROVIDER_BACKEND") {
        Ok(value) if value.eq_ignore_ascii_case("new") => ProviderBackend::New,
        _ => ProviderBackend::Legacy,
    }
}

pub fn providers_repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../greentic-messaging-providers")
}

pub fn providers_components_root() -> PathBuf {
    providers_repo_root().join("packs/messaging-provider-bundle/components")
}

pub fn implementation_origin() -> &'static str {
    "greentic-messaging-providers"
}

pub fn webchat_capabilities() -> CapabilitiesResponseV1 {
    let path = providers_components_root().join("webchat-capabilities_v1.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing capabilities file at {}", path.display()));
    serde_json::from_str(&raw)
        .unwrap_or_else(|err| panic!("failed to parse capabilities json: {err}"))
}

pub fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/vectors")
}

pub fn load_vector_plan(name: &str) -> RenderPlan {
    let path = fixtures_root().join(format!("{name}.json"));
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing vector fixture {}", path.display()));
    serde_json::from_str(&raw)
        .unwrap_or_else(|err| panic!("invalid render plan in {}: {err}", path.display()))
}

pub struct EncodedPayload {
    pub content_type: String,
    pub body: Value,
    pub metadata: Value,
    pub warnings: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
struct CapabilitiesResponseV1 {
    version: String,
    metadata: ProviderMetadataV1,
    capabilities: ProviderCapabilitiesV1,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
struct ProviderMetadataV1 {
    provider_id: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    rate_limit_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
struct ProviderCapabilitiesV1 {
    #[serde(default)]
    supports_webhook_validation: bool,
}

fn parse_body(content_type: &str, body: &[u8]) -> Value {
    if body.is_empty() {
        return Value::Null;
    }
    if content_type.contains("json") {
        serde_json::from_slice(body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(body).into_owned()))
    } else {
        Value::String(String::from_utf8_lossy(body).into_owned())
    }
}

fn parse_metadata(metadata_json: &Option<String>) -> Value {
    if let Some(meta) = metadata_json {
        serde_json::from_str(meta).unwrap_or_else(|_| Value::String(meta.clone()))
    } else {
        Value::Null
    }
}

fn make_engine() -> Engine {
    let mut config = Config::new();
    config.wasm_component_model(true);
    Engine::new(&config).expect("engine")
}

#[derive(Default)]
struct HostState;

mod webchat {
    use super::*;

    mod bindings {
        wasmtime::component::bindgen!({
            path: "../../../greentic-messaging-providers/components/webchat/wit/webchat",
            world: "webchat",
        });
    }

    macro_rules! impl_common_hosts {
        ($bindings:ident) => {
            impl $bindings::greentic::http::http_client::Host for HostState {
                fn send(
                    &mut self,
                    _req: $bindings::greentic::http::http_client::Request,
                    _options: Option<$bindings::greentic::http::http_client::RequestOptions>,
                    _ctx: Option<$bindings::greentic::interfaces_types::types::TenantCtx>,
                ) -> Result<
                    $bindings::greentic::http::http_client::Response,
                    $bindings::greentic::http::http_client::HostError,
                > {
                    Ok($bindings::greentic::http::http_client::Response {
                        status: 200,
                        headers: vec![],
                        body: None,
                    })
                }
            }

            impl $bindings::greentic::secrets_store::secrets_store::Host for HostState {
                fn get(
                    &mut self,
                    _key: String,
                ) -> Result<
                    Option<Vec<u8>>,
                    $bindings::greentic::secrets_store::secrets_store::SecretsError,
                > {
                    Ok(None)
                }
            }

            impl $bindings::greentic::state::state_store::Host for HostState {
                fn read(
                    &mut self,
                    _key: $bindings::greentic::interfaces_types::types::StateKey,
                    _ctx: Option<$bindings::greentic::interfaces_types::types::TenantCtx>,
                ) -> Result<Vec<u8>, $bindings::greentic::state::state_store::HostError> {
                    Err($bindings::greentic::state::state_store::HostError {
                        code: "unimplemented".into(),
                        message: "state not available in tests".into(),
                    })
                }

                fn write(
                    &mut self,
                    _key: $bindings::greentic::interfaces_types::types::StateKey,
                    _bytes: Vec<u8>,
                    _ctx: Option<$bindings::greentic::interfaces_types::types::TenantCtx>,
                ) -> Result<
                    $bindings::greentic::state::state_store::OpAck,
                    $bindings::greentic::state::state_store::HostError,
                > {
                    Err($bindings::greentic::state::state_store::HostError {
                        code: "unimplemented".into(),
                        message: "state not available in tests".into(),
                    })
                }

                fn delete(
                    &mut self,
                    _key: $bindings::greentic::interfaces_types::types::StateKey,
                    _ctx: Option<$bindings::greentic::interfaces_types::types::TenantCtx>,
                ) -> Result<
                    $bindings::greentic::state::state_store::OpAck,
                    $bindings::greentic::state::state_store::HostError,
                > {
                    Err($bindings::greentic::state::state_store::HostError {
                        code: "unimplemented".into(),
                        message: "state not available in tests".into(),
                    })
                }
            }

            impl $bindings::greentic::telemetry::logger_api::Host for HostState {
                fn log(
                    &mut self,
                    _span: $bindings::greentic::interfaces_types::types::SpanContext,
                    _fields: Vec<(String, String)>,
                    _ctx: Option<$bindings::greentic::interfaces_types::types::TenantCtx>,
                ) -> Result<
                    $bindings::greentic::telemetry::logger_api::OpAck,
                    $bindings::greentic::telemetry::logger_api::HostError,
                > {
                    Ok($bindings::greentic::telemetry::logger_api::OpAck::Ok)
                }
            }

            impl $bindings::provider::common::capabilities::Host for HostState {}
            impl $bindings::provider::common::render::Host for HostState {}
            impl $bindings::greentic::interfaces_types::types::Host for HostState {}
        };
    }

    impl_common_hosts!(bindings);

    pub struct Harness {
        store: Store<HostState>,
        bindings: bindings::Webchat,
    }

    impl Harness {
        pub fn new(component_path: &Path) -> Self {
            let engine = make_engine();
            let component =
                Component::from_file(&engine, component_path).expect("load webchat component");
            let mut linker = Linker::new(&engine);
            bindings::Webchat::add_to_linker::<_, HasSelf<HostState>>(
                &mut linker,
                |s: &mut HostState| s,
            )
            .expect("linker");
            let mut store = Store::new(&engine, HostState::default());
            let bindings =
                bindings::Webchat::instantiate(&mut store, &component, &linker).expect("inst");
            bindings
                .call_init_runtime_config(&mut store, "{}".into())
                .expect("init call")
                .expect("init runtime config");
            Self { store, bindings }
        }

        pub fn encode(&mut self, plan: &RenderPlan) -> EncodedPayload {
            let plan = to_bindings_plan(plan);
            let res = self
                .bindings
                .call_encode(&mut self.store, &plan)
                .expect("encode");
            let warnings: Vec<Value> = res
                .warnings
                .into_iter()
                .map(|w| {
                    json!({
                        "code": w.code,
                        "message": w.message,
                        "path": w.path
                    })
                })
                .collect();
            EncodedPayload {
                content_type: res.payload.content_type.clone(),
                body: parse_body(&res.payload.content_type, &res.payload.body),
                metadata: parse_metadata(&res.payload.metadata_json),
                warnings,
            }
        }
    }

    fn to_bindings_plan(plan: &RenderPlan) -> bindings::provider::common::render::RenderPlan {
        bindings::provider::common::render::RenderPlan {
            tier: match plan.tier {
                RenderTier::TierA => bindings::provider::common::render::RenderTier::TierA,
                RenderTier::TierB => bindings::provider::common::render::RenderTier::TierB,
                RenderTier::TierC => bindings::provider::common::render::RenderTier::TierC,
                RenderTier::TierD => bindings::provider::common::render::RenderTier::TierD,
            },
            summary_text: plan.summary_text.clone(),
            actions: plan.actions.clone(),
            attachments: plan.attachments.clone(),
            warnings: plan
                .warnings
                .iter()
                .map(|w| bindings::provider::common::render::RenderWarning {
                    code: w.code.clone(),
                    message: w.message.clone(),
                    path: w.path.clone(),
                })
                .collect(),
            debug_json: plan
                .debug
                .as_ref()
                .map(|d| serde_json::to_string(d).expect("debug json")),
        }
    }
}

pub struct WebchatHarness {
    inner: webchat::Harness,
}

impl WebchatHarness {
    pub fn new() -> Self {
        let component_path = providers_components_root().join("webchat.wasm");
        if !component_path.exists() {
            panic!(
                "webchat component missing at {}; ensure greentic-messaging-providers is checked out",
                component_path.display()
            );
        }
        Self {
            inner: webchat::Harness::new(&component_path),
        }
    }

    pub fn encode(&mut self, plan: &RenderPlan) -> EncodedPayload {
        self.inner.encode(plan)
    }
}

pub fn vector_names() -> Vec<&'static str> {
    vec!["text_simple", "adaptivecard_basic", "adaptivecard_actions"]
}

pub fn build_plan_from_text(summary_text: &str) -> RenderPlan {
    RenderPlan {
        tier: RenderTier::TierD,
        summary_text: Some(summary_text.to_string()),
        actions: Vec::new(),
        attachments: Vec::new(),
        warnings: vec![RenderWarning {
            code: "note".into(),
            message: Some("generated in provider harness".into()),
            path: None,
        }],
        debug: None,
    }
}
