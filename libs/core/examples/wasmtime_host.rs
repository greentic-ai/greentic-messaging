//! Minimal Wasmtime host setup using Greentic host bindings.

#[cfg(feature = "component-host")]
mod demo {
    use anyhow::Result;
    use greentic_interfaces_host::runner_host_v1;
    use gsm_core::add_host_imports;
    use wasmtime::component::Linker;
    use wasmtime::{Config, Engine};

    pub struct DemoHost;

    impl runner_host_v1::RunnerHost for DemoHost {
        fn http_request(
            &mut self,
            _method: String,
            _url: String,
            _headers: Vec<String>,
            _body: Option<Vec<u8>>,
        ) -> wasmtime::Result<Result<Vec<u8>, String>> {
            Ok(Err("http_request unavailable".into()))
        }

        fn kv_get(&mut self, _ns: String, _key: String) -> wasmtime::Result<Option<String>> {
            Ok(None)
        }

        fn kv_put(&mut self, _ns: String, _key: String, _val: String) -> wasmtime::Result<()> {
            Ok(())
        }
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
