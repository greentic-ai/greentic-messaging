use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use tempfile::TempDir;

use crate::pack_io;
use crate::provider_ext;
use gsm_core::pack_extensions::{IngressCapabilities, RuntimeRef};

#[derive(Clone, Debug, Serialize)]
pub struct ProviderInfo {
    pub id: String,
    pub runtime: RuntimeRef,
    pub capabilities: IngressCapabilities,
    pub pack_spec: PathBuf,
    pub pack_root: PathBuf,
}

pub struct ProviderRegistry {
    entries: Vec<ProviderInfo>,
    index: HashMap<String, usize>,
    _temp_dirs: Vec<TempDir>,
}

impl ProviderRegistry {
    pub fn empty() -> Self {
        Self {
            entries: Vec::new(),
            index: HashMap::new(),
            _temp_dirs: Vec::new(),
        }
    }

    pub fn from_pack_paths(paths: &[PathBuf]) -> Result<Self> {
        let mut entries = Vec::new();
        let mut index = HashMap::new();
        let mut temp_dirs = Vec::new();

        for path in paths {
            let materialized = materialize_pack(path)?;
            if let Some(dir) = materialized.temp_dir {
                temp_dirs.push(dir);
            }

            let pack_parent = materialized
                .pack_spec
                .parent()
                .context("pack spec has no parent directory")?
                .to_path_buf();
            let providers = provider_ext::list_messaging_providers(
                &pack_parent,
                std::slice::from_ref(&materialized.pack_spec),
            )?;
            for provider in providers {
                if index.contains_key(&provider.id) {
                    // keep first occurrence
                    continue;
                }
                index.insert(provider.id.clone(), entries.len());
                entries.push(ProviderInfo {
                    id: provider.id,
                    runtime: provider.runtime,
                    capabilities: provider.capabilities,
                    pack_spec: materialized.pack_spec.clone(),
                    pack_root: materialized.pack_root.clone(),
                });
            }
        }

        Ok(Self {
            entries,
            index,
            _temp_dirs: temp_dirs,
        })
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

struct PackMaterialization {
    pack_spec: PathBuf,
    pack_root: PathBuf,
    temp_dir: Option<TempDir>,
}

fn materialize_pack(path: &Path) -> Result<PackMaterialization> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", path.display()))?;
    if canonical.is_dir() {
        let pack_spec = locate_pack_spec(&canonical)?;
        return Ok(PackMaterialization {
            pack_spec,
            pack_root: canonical,
            temp_dir: None,
        });
    }
    if !is_gtpack(&canonical) {
        bail!(
            "provider pack {} must be a directory or .gtpack file",
            canonical.display()
        );
    }
    let extracted = pack_io::extract_pack_to_temp(&canonical)?;
    Ok(PackMaterialization {
        pack_spec: canonical,
        pack_root: extracted.root.clone(),
        temp_dir: Some(extracted.temp_dir),
    })
}

fn is_gtpack(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("gtpack"))
        .unwrap_or(false)
}

fn locate_pack_spec(dir: &Path) -> Result<PathBuf> {
    let pack_yaml = dir.join("pack.yaml");
    if pack_yaml.is_file() {
        return pack_yaml.canonicalize().with_context(|| {
            format!(
                "failed to canonicalize pack manifest {}",
                pack_yaml.display()
            )
        });
    }
    let manifest = dir.join("manifest.cbor");
    if manifest.is_file() {
        return manifest
            .canonicalize()
            .with_context(|| format!("failed to canonicalize manifest {}", manifest.display()));
    }
    bail!(
        "directory {} does not contain pack.yaml or manifest.cbor",
        dir.display()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;

    #[test]
    fn builds_registry_from_fixture_pack() -> Result<()> {
        let pack = fixture_pack();
        let registry = ProviderRegistry::from_pack_paths(std::slice::from_ref(&pack))?;
        assert_eq!(registry.entries().len(), 1);
        let provider = &registry.entries()[0];
        assert_eq!(provider.id, "dev-viewer-provider");
        assert_eq!(provider.pack_spec, pack);
        assert!(provider.pack_root.exists());
        Ok(())
    }

    fn fixture_pack() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/packs/dev-viewer-provider.gtpack")
            .canonicalize()
            .expect("fixture pack exists")
    }
}
