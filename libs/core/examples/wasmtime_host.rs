//! Minimal Wasmtime host setup using Greentic host bindings.

#[cfg(feature = "component-host")]
mod demo {
    use anyhow::Result;
    use greentic_interfaces_host::host_import::v0_6;
    use greentic_interfaces_host::host_import::v0_6::{http, iface_types, state, types};
    use gsm_core::add_host_imports;
    use wasmtime::component::Linker;
    use wasmtime::{Config, Engine};

    pub struct DemoHost;

    impl v0_6::HostImports for DemoHost {
        fn secrets_get(
            &mut self,
            _key: String,
            _ctx: Option<types::TenantCtx>,
        ) -> wasmtime::Result<Result<String, types::IfaceError>> {
            Ok(Err(unavailable("secrets_get")))
        }

        fn telemetry_emit(
            &mut self,
            _span_json: String,
            _ctx: Option<types::TenantCtx>,
        ) -> wasmtime::Result<()> {
            Ok(())
        }

        fn http_fetch(
            &mut self,
            _req: http::HttpRequest,
            _ctx: Option<types::TenantCtx>,
        ) -> wasmtime::Result<Result<http::HttpResponse, types::IfaceError>> {
            Ok(Err(unavailable("http_fetch")))
        }

        fn mcp_exec(
            &mut self,
            _component: String,
            _action: String,
            _args_json: String,
            _ctx: Option<types::TenantCtx>,
        ) -> wasmtime::Result<Result<String, types::IfaceError>> {
            Ok(Err(unavailable("mcp_exec")))
        }

        fn state_get(
            &mut self,
            _key: iface_types::StateKey,
            _ctx: Option<types::TenantCtx>,
        ) -> wasmtime::Result<Result<String, types::IfaceError>> {
            Ok(Err(unavailable("state_get")))
        }

        fn state_set(
            &mut self,
            _key: iface_types::StateKey,
            _value_json: String,
            _ctx: Option<types::TenantCtx>,
        ) -> wasmtime::Result<Result<state::OpAck, types::IfaceError>> {
            Ok(Err(unavailable("state_set")))
        }

        fn session_update(
            &mut self,
            _cursor: iface_types::SessionCursor,
            _ctx: Option<types::TenantCtx>,
        ) -> wasmtime::Result<Result<String, types::IfaceError>> {
            Ok(Err(unavailable("session_update")))
        }
    }

    fn unavailable(_op: &str) -> types::IfaceError {
        types::IfaceError::Unavailable
    }

    pub fn run() -> Result<()> {
        // Enable the component model; this example focuses on wiring the host imports.
        let mut config = Config::new();
        config.wasm_component_model(true);
        let engine = Engine::new(&config)?;

        let mut linker: Linker<DemoHost> = Linker::new(&engine);
        add_host_imports(&mut linker)?;

        // Component loading and instantiation would follow here.
        Ok(())
    }
}

#[cfg(feature = "component-host")]
fn main() -> anyhow::Result<()> {
    demo::run()
}

#[cfg(not(feature = "component-host"))]
fn main() {
    eprintln!("Enable `component-host` feature to run this example.");
}
