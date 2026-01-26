use std::cmp::Ordering;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::Context;
use greentic_types::decode_pack_manifest;
use greentic_types::pack_manifest::PackManifest;
use greentic_types::provider::ProviderDecl;
use serde::Serialize;
use tempfile::TempDir;

use crate::pack_io;
use crate::provider_ext;
use gsm_core::pack_extensions::{IngressCapabilities, RuntimeRef};

/// Errors that occur while materializing a provider pack.
#[derive(Clone, Debug, Serialize)]
pub struct PackLoadError {
    pub path: PathBuf,
    pub reason: String,
}

impl PackLoadError {
    fn new(path: &Path, reason: impl Into<String>) -> Self {
        Self {
            path: path.to_path_buf(),
            reason: reason.into(),
        }
    }
}

impl std::fmt::Display for PackLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.reason)
    }
}

/// Represents a provider extracted from a pack manifest.
pub struct LoadedProvider {
    pub id: String,
    pub runtime: RuntimeRef,
    pub capabilities: Vec<String>,
    pub pack_id: String,
}

/// A pack that was successfully materialized and had providers extracted.
pub struct LoadedPack {
    pub pack_spec: PathBuf,
    pub pack_root: PathBuf,
    pub providers: Vec<LoadedProvider>,
    pub temp_dir: Option<TempDir>,
}

/// Discover `.gtpack` files from explicit paths or directories.
pub fn discover_pack_paths(
    explicit: &[PathBuf],
    packs_dirs: &[PathBuf],
) -> anyhow::Result<Vec<PathBuf>> {
    let mut discovered = Vec::new();
    for path in explicit {
        discovered.push(canonicalize_path(path));
    }

    for dir in packs_dirs {
        for entry in fs::read_dir(dir)
            .with_context(|| format!("failed to read packs directory {}", dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if is_gtpack(&path) {
                discovered.push(canonicalize_path(&path));
            }
        }
    }

    discovered.sort_by(|a, b| match (a.file_name(), b.file_name()) {
        (Some(a), Some(b)) => a.cmp(b),
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    });
    discovered.dedup();
    Ok(discovered)
}

/// Load each requested pack, returning successful packs and the errors that occurred.
pub fn load_packs(paths: &[PathBuf]) -> (Vec<LoadedPack>, Vec<PackLoadError>) {
    let mut loaded = Vec::new();
    let mut errors = Vec::new();
    for path in paths {
        match load_pack(path) {
            Ok(pack) => loaded.push(pack),
            Err(err) => errors.push(err),
        }
    }
    (loaded, errors)
}

fn load_pack(path: &Path) -> Result<LoadedPack, PackLoadError> {
    let canonical = path
        .canonicalize()
        .map_err(|err| PackLoadError::new(path, format!("failed to canonicalize: {err}")))?;

    let (pack_root, pack_spec, manifest_path, temp_dir) = if canonical.is_dir() {
        let pack_spec =
            locate_pack_spec(&canonical).map_err(|err| PackLoadError::new(&canonical, err))?;
        let manifest_path = pack_spec.clone();
        (canonical.clone(), pack_spec, manifest_path, None)
    } else if is_gtpack(&canonical) {
        let extracted = pack_io::extract_pack_to_temp(&canonical)
            .map_err(|err| PackLoadError::new(&canonical, err.to_string()))?;
        let manifest_path = extracted.root.join("manifest.cbor");
        (
            extracted.root.clone(),
            canonical.clone(),
            manifest_path,
            Some(extracted.temp_dir),
        )
    } else {
        return Err(PackLoadError::new(
            &canonical,
            "provider pack must be a directory or .gtpack file",
        ));
    };

    if !manifest_path.exists() {
        return Err(PackLoadError::new(
            &manifest_path,
            "pack manifest not found",
        ));
    }

    let manifest = read_manifest(&manifest_path)?;
    let pack_id = manifest.pack_id.to_string();
    let mut providers = extract_providers_from_manifest(&manifest, &pack_id);

    if providers.is_empty() {
        let pack_parent = pack_spec
            .parent()
            .map(PathBuf::from)
            .ok_or_else(|| PackLoadError::new(&pack_spec, "pack spec has no parent directory"))?;
        let entries =
            provider_ext::list_messaging_providers(&pack_parent, std::slice::from_ref(&pack_spec))
                .map_err(|err| PackLoadError::new(&pack_spec, err.to_string()))?;
        providers = entries
            .into_iter()
            .map(|entry| LoadedProvider {
                id: entry.id,
                runtime: entry.runtime,
                capabilities: ingress_capabilities_to_vec(&entry.capabilities),
                pack_id: pack_id.clone(),
            })
            .collect();
    }

    if providers.is_empty() {
        return Err(PackLoadError::new(
            &manifest_path,
            "no messaging providers declared in pack",
        ));
    }

    Ok(LoadedPack {
        pack_spec,
        pack_root,
        providers,
        temp_dir,
    })
}

fn read_manifest(path: &Path) -> Result<PackManifest, PackLoadError> {
    let mut buf = Vec::new();
    let mut file = fs::File::open(path)
        .map_err(|err| PackLoadError::new(path, format!("failed to open manifest: {err}")))?;
    file.read_to_end(&mut buf)
        .map_err(|err| PackLoadError::new(path, format!("failed to read manifest: {err}")))?;
    decode_pack_manifest(&buf).map_err(|err| PackLoadError::new(path, err.to_string()))
}

fn extract_providers_from_manifest(manifest: &PackManifest, pack_id: &str) -> Vec<LoadedProvider> {
    manifest
        .provider_extension_inline()
        .map(|inline| {
            inline
                .providers
                .iter()
                .map(|decl| provider_decl_to_loaded(decl, pack_id))
                .collect()
        })
        .unwrap_or_default()
}

fn provider_decl_to_loaded(decl: &ProviderDecl, pack_id: &str) -> LoadedProvider {
    LoadedProvider {
        id: decl.provider_type.clone(),
        runtime: RuntimeRef {
            component_ref: decl.runtime.component_ref.clone(),
            export: decl.runtime.export.clone(),
            world: decl.runtime.world.clone(),
        },
        capabilities: decl.capabilities.clone(),
        pack_id: pack_id.to_string(),
    }
}

fn ingress_capabilities_to_vec(cap: &IngressCapabilities) -> Vec<String> {
    let mut values = Vec::new();
    if cap.supports_webhook_validation {
        values.push("webhook-validation".into());
    }
    if !cap.content_types.is_empty() {
        values.push(format!("content-types: {}", cap.content_types.join(", ")));
    }
    values
}

fn canonicalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn is_gtpack(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("gtpack"))
        .unwrap_or(false)
}

fn locate_pack_spec(dir: &Path) -> Result<PathBuf, String> {
    let pack_yaml = dir.join("pack.yaml");
    if pack_yaml.is_file() {
        return pack_yaml.canonicalize().map_err(|err| {
            format!(
                "failed to canonicalize pack manifest {}: {err}",
                pack_yaml.display()
            )
        });
    }
    let manifest = dir.join("manifest.cbor");
    if manifest.is_file() {
        return manifest.canonicalize().map_err(|err| {
            format!(
                "failed to canonicalize manifest {}: {err}",
                manifest.display()
            )
        });
    }
    Err(format!(
        "directory {} does not contain pack.yaml or manifest.cbor",
        dir.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::{ZipWriter, write::FileOptions};

    fn create_gtpack(path: &Path) -> anyhow::Result<()> {
        let file = std::fs::File::create(path)?;
        let mut writer = ZipWriter::new(file);
        let options: FileOptions<'_, ()> = FileOptions::default();
        writer.start_file("manifest.cbor", options)?;
        writer.write_all(b"manifest")?;
        writer.start_file("contents/data.txt", options)?;
        writer.write_all(b"value")?;
        writer.finish()?;
        Ok(())
    }

    #[test]
    fn discover_pack_paths_filters_and_sorts() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let a = dir.path().join("zulu.gtpack");
        let b = dir.path().join("alpha.gtpack");
        create_gtpack(&a)?;
        create_gtpack(&b)?;
        let explicit = vec![a.clone()];
        let discovered = discover_pack_paths(&explicit, &[dir.path().to_path_buf()])?;
        let expected = vec![canonicalize_path(&b), canonicalize_path(&a)];
        assert_eq!(discovered, expected);
        Ok(())
    }

    #[test]
    fn load_pack_reports_provider() -> anyhow::Result<()> {
        let pack = fixture_pack();
        let (packs, errors) = load_packs(std::slice::from_ref(&pack));
        assert!(errors.is_empty());
        assert_eq!(packs.len(), 1);
        assert!(!packs[0].providers.is_empty());
        Ok(())
    }

    fn fixture_pack() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/packs/dev-viewer-provider.gtpack")
            .canonicalize()
            .expect("fixture pack exists")
    }
}
