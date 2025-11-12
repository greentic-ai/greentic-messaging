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
        let card = if is_adaptive_card(&value) {
            AdaptiveMessageCard {
                adaptive: Some(value),
                ..Default::default()
            }
        } else {
            serde_json::from_value(value)?
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
