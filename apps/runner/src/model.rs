use anyhow::{Context, bail};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize)]
pub struct Flow {
    pub id: String,
    #[allow(dead_code)]
    pub title: Option<String>,
    #[allow(dead_code)]
    pub description: Option<String>,
    #[serde(rename = "type")]
    #[allow(dead_code)]
    pub kind: String,
    #[serde(rename = "in")]
    pub r#in: String,
    pub nodes: BTreeMap<String, Node>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Node {
    #[serde(default)]
    pub qa: Option<QaNode>,
    #[serde(default)]
    pub tool: Option<ToolNode>,
    #[serde(default)]
    pub template: Option<TemplateNode>,
    #[serde(default)]
    pub card: Option<CardNode>,
    #[serde(default)]
    pub routes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QaNode {
    #[allow(dead_code)]
    pub welcome: Option<String>,
    pub questions: Vec<Question>,
    #[serde(default)]
    pub fallback_agent: Option<AgentCfg>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Question {
    pub id: String,
    #[allow(dead_code)]
    pub prompt: String,
    #[serde(default)]
    pub answer_type: Option<String>,
    #[serde(default)]
    pub max_words: Option<usize>,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    #[serde(default)]
    pub validate: Option<Validate>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Validate {
    pub range: Option<[f64; 2]>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentCfg {
    #[serde(default)]
    pub r#type: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    pub task: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolNode {
    pub tool: String,
    pub action: String,
    #[serde(default)]
    pub input: serde_json::Value,
    #[serde(default)]
    pub retry: Option<u32>,
    #[serde(default)]
    pub delay_secs: Option<u64>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TemplateNode {
    pub template: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CardNode {
    pub title: Option<String>,
    #[serde(default)]
    pub body: Vec<CardBlock>,
    #[serde(default)]
    pub actions: Vec<CardAction>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum CardBlock {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(default)]
        markdown: Option<bool>,
    },
    #[serde(rename = "fact")]
    Fact { label: String, value: String },
    #[serde(rename = "image")]
    Image { url: String },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum CardAction {
    #[serde(rename = "openUrl")]
    OpenUrl {
        title: String,
        url: String,
        #[serde(default)]
        jwt: Option<bool>,
    },
    #[serde(rename = "postback")]
    Postback {
        title: String,
        data: serde_json::Value,
    },
}

impl Flow {
    pub fn load_from_file(path: &str) -> anyhow::Result<Self> {
        let txt = std::fs::read_to_string(path)
            .with_context(|| format!("reading flow definition at {path}"))?;
        let flow: Flow = serde_yaml_bw::from_str(&txt)
            .with_context(|| format!("parsing flow yaml at {path}"))?;
        flow.validate()?;
        Ok(flow)
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.id.trim().is_empty() {
            bail!("flow missing id");
        }
        if self.kind.trim().is_empty() {
            bail!("flow {} missing type", self.id);
        }
        if self.r#in.trim().is_empty() {
            bail!("flow {} missing entry point `in`", self.id);
        }
        if self.nodes.is_empty() {
            bail!("flow {} defines no nodes", self.id);
        }
        if !self.nodes.contains_key(&self.r#in) {
            bail!(
                "flow {} entry point `{}` not found in nodes",
                self.id,
                self.r#in
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};

    fn write_temp_flow(contents: &str) -> PathBuf {
        let suffix = uuid::Uuid::new_v4();
        let path = std::env::temp_dir().join(format!("flow-{suffix}.yaml"));
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn load_from_file_parses_flow_structure() {
        let yaml = r#"
id: flow-1
title: Sample Flow
type: qa
in: start
nodes:
  start:
    qa:
      welcome: "Hello"
      questions:
        - id: q1
          prompt: "What is your name?"
    routes: []
"#;
        let path = write_temp_flow(yaml);
        let flow = Flow::load_from_file(path.to_str().unwrap()).expect("flow");
        fs::remove_file(path).ok();

        assert_eq!(flow.id, "flow-1");
        assert_eq!(flow.kind, "qa");
        let start = flow.nodes.get("start").expect("start node");
        let qa = start.qa.as_ref().expect("qa node");
        assert_eq!(qa.questions.len(), 1);
        assert_eq!(qa.questions[0].prompt, "What is your name?");
    }

    #[test]
    fn missing_optional_sections_default() {
        let yaml = r#"
id: flow-2
type: tool
in: worker
nodes:
  worker:
    tool:
      tool: weather
      action: fetch
    routes: []
"#;
        let path = write_temp_flow(yaml);
        let flow = Flow::load_from_file(path.to_str().unwrap()).expect("flow");
        fs::remove_file(path).ok();

        let worker = flow.nodes.get("worker").expect("worker node");
        assert!(worker.qa.is_none());
        assert!(worker.card.is_none());
        assert_eq!(worker.routes, Vec::<String>::new());
    }

    #[test]
    fn load_from_file_errors_on_invalid_yaml() {
        let yaml = r#"
id: flow-3
type: qa
nodes:
  start:
    qa: {}
        "#;
        let path = write_temp_flow(yaml);
        let result = Flow::load_from_file(path.to_str().unwrap());
        fs::remove_file(path).ok();
        assert!(result.is_err());
    }
}
