use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use gsm_core::AdaptiveMessageCard;
use serde_json::Value;

pub struct Fixture {
    pub id: String,
    pub path: PathBuf,
    pub card: AdaptiveMessageCard,
}

pub fn discover(root: &Path) -> Result<Vec<Fixture>> {
    let mut fixtures = Vec::new();
    for entry in fs::read_dir(root).context("failed to read fixtures dir")? {
        let entry = entry.context("dir entry")?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path.extension().and_then(|s| s.to_str());
        if !matches!(ext, Some("json") | Some("yaml") | Some("yml")) {
            continue;
        }
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unnamed")
            .to_string();
        let data = fs::read_to_string(&path).context("read fixture")?;
        let value: Value = if matches!(ext, Some("yaml") | Some("yml")) {
            serde_yaml::from_str(&data)?
        } else {
            serde_json::from_str(&data)?
        };
        let mut normalized = value.clone();
        normalize_adaptive(&mut normalized);
        let card = if is_adaptive_card(&normalized) {
            AdaptiveMessageCard {
                adaptive: Some(normalized),
                ..Default::default()
            }
        } else {
            serde_json::from_value(normalized)?
        };
        fixtures.push(Fixture { id, path, card });
    }
    fixtures.sort_by_key(|f| f.id.clone());
    Ok(fixtures)
}

fn is_adaptive_card(value: &Value) -> bool {
    value
        .get("type")
        .and_then(|v| v.as_str())
        .map(|t| t == "AdaptiveCard")
        .unwrap_or(false)
}

fn normalize_adaptive(value: &mut Value) {
    ensure_version(value);
    match value {
        Value::Object(map) => {
            for val in map.values_mut() {
                normalize_adaptive(val);
            }
        }
        Value::Array(arr) => {
            for val in arr {
                normalize_adaptive(val);
            }
        }
        _ => {}
    }
}

fn ensure_version(value: &mut Value) {
    if let Some(map) = value.as_object_mut()
        && map.get("type").and_then(|v| v.as_str()) == Some("AdaptiveCard")
        && !map.contains_key("version")
    {
        map.insert("version".into(), Value::String("1.6".into()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn discover_loads_json_fixtures() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("card.json");
        let mut file = fs::File::create(&path).unwrap();
        write!(
            file,
            "{}",
            json!({
                "$schema": "http://adaptivecards.io/schemas/adaptive-card.json",
                "type": "AdaptiveCard",
                "version": "1.6",
                "body": []
            })
        )
        .unwrap();
        let fixtures = discover(dir.path()).expect("discover");
        assert_eq!(fixtures.len(), 1);
        assert_eq!(fixtures[0].id, "card");
    }
}
