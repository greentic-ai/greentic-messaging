//! Wasmtime linker helpers for the Greentic host bindings.
use anyhow::Result;
use greentic_interfaces_host::host_import::v0_6;
use wasmtime::component::Linker;

/// Registers the Greentic host imports with the provided Wasmtime linker.
///
/// The store data `T` must implement the generated `HostImports` trait so the linker
/// can call back into your host implementation.
pub fn add_host_imports<T>(linker: &mut Linker<T>) -> Result<()>
where
    T: v0_6::HostImports + Send + Sync + 'static,
{
    v0_6::add_to_linker(linker, |host| host)
}
