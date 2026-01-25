use anyhow::Context;
use anyhow::Result;
use serde::Serialize;
use std::path::{Path, PathBuf};

use gsm_core::pack_extensions::{
    IngressCapabilities, RuntimeRef, load_provider_extensions_from_pack_files,
};

#[derive(Clone, Debug, Serialize)]
pub struct ProviderEntry {
    pub id: String,
    pub runtime: RuntimeRef,
    pub capabilities: IngressCapabilities,
    pub pack_path: PathBuf,
}

pub fn list_messaging_providers(root: &Path, pack_paths: &[PathBuf]) -> Result<Vec<ProviderEntry>> {
    let mut providers = Vec::new();
    for pack_path in pack_paths {
        let registry =
            load_provider_extensions_from_pack_files(root, std::slice::from_ref(pack_path))
                .with_context(|| {
                    format!(
                        "failed to load provider extensions from {}",
                        pack_path.display()
                    )
                })?;
        for (id, decl) in registry.ingress {
            providers.push(ProviderEntry {
                id,
                runtime: decl.runtime,
                capabilities: decl.capabilities,
                pack_path: pack_path.clone(),
            });
        }
    }
    Ok(providers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_providers_from_pack_fixture() -> Result<()> {
        let fixture = fixture_pack();
        let root = fixture.parent().expect("fixture has parent").to_path_buf();
        let providers = list_messaging_providers(&root, std::slice::from_ref(&fixture))?;
        assert_eq!(providers.len(), 1);
        let provider = &providers[0];
        assert_eq!(provider.id, "dev-viewer-provider");
        assert_eq!(provider.runtime.component_ref, "dev-viewer.component");
        assert!(provider.capabilities.supports_webhook_validation);
        assert_eq!(provider.pack_path, fixture);
        Ok(())
    }

    fn fixture_pack() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/packs/dev-viewer-provider.gtpack")
            .canonicalize()
            .expect("fixture pack exists")
    }
}
