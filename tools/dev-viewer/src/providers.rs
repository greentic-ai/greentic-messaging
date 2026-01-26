use std::collections::HashMap;
use std::path::PathBuf;

use serde::Serialize;
use tempfile::TempDir;
use tracing::warn;

use crate::pack_loader::LoadedPack;
use gsm_core::pack_extensions::RuntimeRef;

#[derive(Clone, Debug, Serialize)]
pub struct ProviderInfo {
    pub id: String,
    pub provider_type: String,
    pub runtime: RuntimeRef,
    pub capabilities: Vec<String>,
    pub pack_id: String,
    pub pack_spec: PathBuf,
    pub pack_root: PathBuf,
}

pub struct ProviderRegistry {
    entries: Vec<ProviderInfo>,
    index: HashMap<String, usize>,
    _temp_dirs: Vec<TempDir>,
}

impl ProviderRegistry {
    pub fn from_loaded_packs(loaded: Vec<LoadedPack>) -> Self {
        let mut entries: Vec<ProviderInfo> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut temp_dirs = Vec::new();

        for pack in loaded {
            if let Some(dir) = pack.temp_dir {
                temp_dirs.push(dir);
            }
            for provider in pack.providers {
                if let Some(existing_idx) = index.get(&provider.pack_id) {
                    let kept_pack = &entries[*existing_idx].pack_spec;
                    warn!(
                        provider_id = %provider.pack_id,
                        provider_type = %provider.id,
                        kept_pack = %kept_pack.display(),
                        duplicate_pack = %pack.pack_spec.display(),
                        "duplicate provider id, keeping first occurrence",
                    );
                    continue;
                }
                index.insert(provider.pack_id.clone(), entries.len());
                entries.push(ProviderInfo {
                    id: provider.pack_id.clone(),
                    provider_type: provider.id,
                    runtime: provider.runtime,
                    capabilities: provider.capabilities,
                    pack_spec: pack.pack_spec.clone(),
                    pack_root: pack.pack_root.clone(),
                    pack_id: provider.pack_id.clone(),
                });
            }
        }

        Self {
            entries,
            index,
            _temp_dirs: temp_dirs,
        }
    }

    pub fn entries(&self) -> &[ProviderInfo] {
        &self.entries
    }

    pub fn get(&self, id: &str) -> Option<&ProviderInfo> {
        self.index.get(id).map(|idx| &self.entries[*idx])
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack_loader::load_packs;
    use std::path::Path;

    #[test]
    fn builds_registry_from_fixture_pack() {
        let pack = fixture_pack();
        let (loaded, errors) = load_packs(std::slice::from_ref(&pack));
        assert!(errors.is_empty());
        let registry = ProviderRegistry::from_loaded_packs(loaded);
        assert_eq!(registry.entries().len(), 1);
        let provider = &registry.entries()[0];
        assert_eq!(provider.id, provider.pack_id);
        assert_eq!(provider.pack_spec, pack);
        assert!(provider.pack_root.exists());
    }

    fn fixture_pack() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/packs/dev-viewer-provider.gtpack")
            .canonicalize()
            .expect("fixture pack exists")
    }
}
