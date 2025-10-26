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
            let number_re = Regex::new(r"(?P<n>\d+)").unwrap();
            for q in &cfg.questions {
                if missing.contains(&q.id.as_str()) {
                    match q.answer_type.as_deref() {
                        Some("number") => {
                            if let Some(c) = number_re.captures(text) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{QaNode, Question, Validate};
    use handlebars::Handlebars;

    fn handlebars() -> &'static Handlebars<'static> {
        Box::leak(Box::new(Handlebars::new()))
    }

    fn envelope_with_text(text: Option<&str>) -> MessageEnvelope {
        MessageEnvelope {
            tenant: "acme".into(),
            platform: gsm_core::Platform::Slack,
            chat_id: "C1".into(),
            user_id: "U1".into(),
            thread_id: None,
            msg_id: "msg-1".into(),
            text: text.map(|t| t.to_string()),
            timestamp: "2024-01-01T00:00:00Z".into(),
            context: Default::default(),
        }
    }

    fn number_question() -> Question {
        Question {
            id: "quantity".into(),
            prompt: "How many?".into(),
            answer_type: Some("number".into()),
            max_words: None,
            default: None,
            validate: Some(Validate {
                range: Some([1.0, 10.0]),
            }),
        }
    }

    #[tokio::test]
    async fn run_qa_populates_defaults_and_text() {
        let qa = QaNode {
            welcome: None,
            questions: vec![
                Question {
                    id: "name".into(),
                    prompt: "Name".into(),
                    answer_type: None,
                    max_words: Some(3),
                    default: Some(json!("guest")),
                    validate: None,
                },
                number_question(),
            ],
            fallback_agent: None,
        };

        let env = envelope_with_text(Some("I need 4 adapters"));
        let mut state = serde_json::Value::Null;
        run_qa(&qa, &env, &mut state, handlebars()).await.unwrap();

        let obj = state.as_object().expect("state object");
        assert_eq!(obj.get("name"), Some(&json!("guest")));
        assert_eq!(obj.get("quantity"), Some(&json!(4.0)));
    }

    #[tokio::test]
    async fn run_qa_errors_when_max_words_exceeded() {
        let qa = QaNode {
            welcome: None,
            questions: vec![Question {
                id: "desc".into(),
                prompt: "Describe".into(),
                answer_type: None,
                max_words: Some(1),
                default: None,
                validate: None,
            }],
            fallback_agent: None,
        };

        let env = envelope_with_text(None);
        let mut state = json!({"desc": "too many words"});
        let err = run_qa(&qa, &env, &mut state, handlebars())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("max_words"));
    }
}
