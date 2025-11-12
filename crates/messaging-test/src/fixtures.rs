use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use gsm_core::MessageCard;

pub struct Fixture {
    pub id: String,
    pub path: PathBuf,
    pub card: MessageCard,
}

pub fn discover(root: &Path) -> Result<Vec<Fixture>> {
    let mut fixtures = Vec::new();
    for entry in fs::read_dir(root).context("failed to read fixtures dir")? {
        let entry = entry.context("dir entry")?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
            if ext != "json" && ext != "yaml" && ext != "yml" {
                continue;
            }
        } else {
            continue;
        }
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unnamed")
            .to_string();
        let data = fs::read_to_string(&path).context("read fixture")?;
        let card = if path.extension().and_then(|s| s.to_str()) == Some("yaml") {
            serde_yaml::from_str(&data)?
        } else {
            serde_json::from_str(&data)?
        };
        fixtures.push(Fixture { id, path, card });
    }
    fixtures.sort_by_key(|f| f.id.clone());
    Ok(fixtures)
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
