use crate::model::{AgentCfg, QaNode};
use anyhow::{bail, Result};
use gsm_core::MessageEnvelope;
use regex::Regex;
use serde_json::{json, Value};

pub async fn run_qa(
    cfg: &QaNode,
    env: &MessageEnvelope,
    state: &mut Value,
    _hbs: &handlebars::Handlebars<'static>,
) -> Result<()> {
    if !state.is_object() {
        *state = json!({});
    }
    let obj = state.as_object_mut().unwrap();

    // Defaults
    for q in &cfg.questions {
        if !obj.contains_key(&q.id) {
            if let Some(def) = &q.default {
                obj.insert(q.id.clone(), def.clone());
            }
        }
    }

    let mut missing: Vec<&str> = cfg
        .questions
        .iter()
        .filter(|q| !obj.contains_key(&q.id))
        .map(|q| q.id.as_str())
        .collect();

    if !missing.is_empty() {
        if let Some(text) = &env.text {
            for q in &cfg.questions {
                if missing.contains(&q.id.as_str()) {
                    match q.answer_type.as_deref() {
                        Some("number") => {
                            let re = Regex::new(r"(?P<n>\d+)").unwrap();
                            if let Some(c) = re.captures(text) {
                                if let Some(m) = c.name("n") {
                                    obj.insert(
                                        q.id.clone(),
                                        json!(m.as_str().parse::<i64>().unwrap_or(1)),
                                    );
                                }
                            }
                        }
                        _ => {
                            let loc = text
                                .split_whitespace()
                                .take(q.max_words.unwrap_or(3))
                                .collect::<Vec<_>>()
                                .join(" ");
                            if !loc.is_empty() {
                                obj.insert(q.id.clone(), json!(loc));
                            }
                        }
                    }
                }
            }
            missing = cfg
                .questions
                .iter()
                .filter(|q| !obj.contains_key(&q.id))
                .map(|q| q.id.as_str())
                .collect();
        }
    }

    if !missing.is_empty() {
        if let Some(agent) = &cfg.fallback_agent {
            let extracted = call_agent(agent, env).await?;
            for (k, v) in extracted.as_object().unwrap_or(&serde_json::Map::new()) {
                obj.insert(k.clone(), v.clone());
            }
        }
    }

    // Validate
    for q in &cfg.questions {
        if let Some(val) = obj.get(&q.id).cloned() {
            if let Some(r) = q.validate.as_ref().and_then(|v| v.range) {
                let n = val
                    .as_f64()
                    .or_else(|| val.as_i64().map(|x| x as f64))
                    .unwrap_or(0.0);
                let clamped = n.clamp(r[0], r[1]);
                obj.insert(q.id.clone(), json!(clamped));
            }
            if let Some(maxw) = q.max_words {
                let s = val.as_str().unwrap_or_default();
                if s.split_whitespace().count() > maxw {
                    bail!("answer '{}' exceeds max_words {}", q.id, maxw);
                }
            }
        }
    }
    Ok(())
}

async fn call_agent(agent: &AgentCfg, env: &MessageEnvelope) -> Result<serde_json::Value> {
    let url = agent
        .endpoint
        .clone()
        .unwrap_or_else(|| "http://localhost:18080/agent/extract".into());
    let body = json!({
      "type": agent.r#type.as_deref().unwrap_or("ollama"),
      "model": agent.model.as_deref().unwrap_or("gemma:instruct"),
      "task": agent.task.as_deref().unwrap_or("extract keys"),
      "text": env.text,
    });
    let resp = reqwest::Client::new().post(url).json(&body).send().await?;
    let v: serde_json::Value = resp.json().await.unwrap_or_else(|_| json!({}));
    Ok(v.get("extracted").cloned().unwrap_or_else(|| json!({})))
}
