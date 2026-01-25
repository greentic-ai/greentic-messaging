use std::collections::BTreeMap;
use std::env;
use std::fs::{create_dir_all, File};
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use greentic_types::pack_manifest::{ExtensionInline, ExtensionRef, PackKind, PackManifest, PackSignatures};
use greentic_types::{encode_pack_manifest, PackId};
use semver::Version;
use serde_json::json;
use zip::write::FileOptions;

fn main() -> Result<()> {
    let output = env::args()
        .nth(1)
        .unwrap_or_else(|| "tests/fixtures/packs/dev-viewer-provider.gtpack".into());
    let output: PathBuf = output.into();
    let parent = output
        .parent()
        .context("fixture path must have a parent directory")?;
    create_dir_all(parent)
        .with_context(|| format!("failed to create fixture directory {}", parent.display()))?;

    let payload = json!({
        "dev-viewer-provider": {
            "runtime": {
                "component_ref": "dev-viewer.component",
                "export": "run",
                "world": "messaging:provider@v1"
            },
            "capabilities": {
                "supports_webhook_validation": true,
                "content_types": ["application/json"]
            }
        }
    });

    let mut extensions = BTreeMap::new();
    extensions.insert(
        "messaging.provider_ingress.v1".to_string(),
        ExtensionRef {
            kind: "messaging.provider_ingress.v1".to_string(),
            version: "1.0.0".to_string(),
            digest: None,
            location: None,
            inline: Some(ExtensionInline::Other(payload)),
        },
    );

    let manifest = PackManifest {
        schema_version: "pack-v1".into(),
        pack_id: PackId::new("dev-viewer.provider").context("invalid pack id")?,
        version: Version::new(0, 1, 0),
        kind: PackKind::Provider,
        publisher: "greentic".into(),
        components: Vec::new(),
        flows: Vec::new(),
        dependencies: Vec::new(),
        capabilities: Vec::new(),
        secret_requirements: Vec::new(),
        signatures: PackSignatures::default(),
        bootstrap: None,
        extensions: Some(extensions),
    };

    let manifest_bytes = encode_pack_manifest(&manifest)
        .context("failed to encode pack manifest for fixture")?;
    let file = File::create(&output)
        .with_context(|| format!("failed to create fixture {}", output.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let options: FileOptions<'_, ()> = FileOptions::default();
    zip.start_file("manifest.cbor", options)?;
    zip.write_all(&manifest_bytes)?;
    zip.finish()?;

    println!("Wrote fixture at {}", output.display());
    Ok(())
}
