use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde_yaml_bw::Value;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf()
}

#[test]
fn messaging_packs_have_flows_and_seed_coverage() -> Result<()> {
    let root = workspace_root();
    let packs_dir = root.join("packs/messaging");
    let flows_dir = root.join("flows/messaging");
    let seed_path = root.join("fixtures/seeds/messaging_all_smoke.yaml");

    let seed_doc: Value = serde_yaml_bw::from_str(
        &fs::read_to_string(&seed_path)
            .with_context(|| format!("failed to read seed {}", seed_path.display()))?,
    )?;
    let seed_uris: HashSet<String> = seed_doc
        .get("entries")
        .and_then(|v| v.as_sequence())
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            entry
                .get("uri")
                .and_then(|u| u.as_str())
                .map(str::to_string)
        })
        .collect();

    for entry in fs::read_dir(&packs_dir)
        .with_context(|| format!("failed to read {}", packs_dir.display()))?
    {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            || entry.path().extension().and_then(|e| e.to_str()) != Some("yaml")
        {
            continue;
        }
        let pack_path = entry.path();
        let pack: Value = serde_yaml_bw::from_str(
            &fs::read_to_string(&pack_path)
                .with_context(|| format!("failed to read pack {}", pack_path.display()))?,
        )?;

        let secret_requirements = pack
            .get("secret_requirements")
            .and_then(|v| v.as_sequence())
            .cloned()
            .unwrap_or_default();

        for req in secret_requirements {
            let key = req.get("key").and_then(|k| k.as_str()).unwrap_or_default();
            let scope = req
                .get("scope")
                .and_then(|s| s.as_mapping())
                .cloned()
                .unwrap_or_default();
            let env = scope
                .get(Value::from("env"))
                .and_then(|v| v.as_str())
                .unwrap_or("dev");
            let tenant = scope
                .get(Value::from("tenant"))
                .and_then(|v| v.as_str())
                .unwrap_or("example");
            let team_val = scope.get(Value::from("team")).and_then(|v| v.as_str());
            let team = team_val.unwrap_or("_");
            let uri = format!("secrets://{env}/{tenant}/{team}/{key}");
            assert!(
                seed_uris.contains(&uri),
                "seed file missing uri {} for requirement {}",
                uri,
                pack_path.display()
            );
        }

        let adapters = pack
            .get("messaging")
            .and_then(|m| m.get("adapters"))
            .and_then(|a| a.as_sequence())
            .cloned()
            .unwrap_or_default();

        for adapter in adapters {
            for field in &["default_flow", "custom_flow"] {
                if let Some(flow_path) = adapter.get(*field).and_then(|f| f.as_str()) {
                    let flow_file = flows_dir.join(
                        flow_path
                            .strip_prefix("flows/messaging/")
                            .unwrap_or(flow_path),
                    );
                    assert!(
                        flow_file.exists(),
                        "flow file {} referenced by {} not found",
                        flow_path,
                        pack_path.display()
                    );
                    let flow_doc: Value = serde_yaml_bw::from_str(
                        &fs::read_to_string(&flow_file).with_context(|| {
                            format!(
                                "failed to read flow {} referenced by {}",
                                flow_file.display(),
                                pack_path.display()
                            )
                        })?,
                    )?;
                    assert!(
                        flow_doc.get("id").and_then(|v| v.as_str()).is_some(),
                        "flow {} missing id",
                        flow_file.display()
                    );
                }
            }
        }
    }

    Ok(())
}
