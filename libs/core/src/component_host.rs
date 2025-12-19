//! Wasmtime linker helpers for the Greentic host bindings.
use anyhow::Result;
use greentic_interfaces_host::runner_host_v1;
use wasmtime::component::Linker;

/// Registers the Greentic host imports with the provided Wasmtime linker.
///
/// The store data `T` must implement the generated `HostImports` trait so the linker
/// can call back into your host implementation.
pub fn add_host_imports<T>(linker: &mut Linker<T>) -> Result<()>
where
    T: runner_host_v1::RunnerHost + Send + Sync + 'static,
{
    runner_host_v1::add_to_linker(linker, |host| host).map_err(Into::into)
}
