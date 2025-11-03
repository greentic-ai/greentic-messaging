use anyhow::{Result, anyhow};
use handlebars::Handlebars;
use rand::{Rng, rng};
use serde_json::{Value, json};
use tokio::time::{Duration, sleep};

use gsm_core::MessageEnvelope;

pub async fn run_tool(
    cfg: &crate::model::ToolNode,
    env: &MessageEnvelope,
    state: &Value,
) -> Result<Value> {
    let mut input = cfg.input.clone();
    render_json_strings(&mut input, &json!({"state":state, "envelope":env}))?;

    let endpoint =
        std::env::var("TOOL_ENDPOINT").unwrap_or_else(|_| "http://localhost:18081".into());
    let url = format!(
        "{}/{}/{}",
        endpoint.trim_end_matches('/'),
        cfg.tool,
        cfg.action
    );

    let retries = cfg.retry.unwrap_or(1);
    let timeout = cfg.timeout_secs.unwrap_or(10);
    let base = cfg.delay_secs.unwrap_or(1);

    for attempt in 0..=retries {
        let resp = tokio::time::timeout(Duration::from_secs(timeout), async {
            reqwest::Client::new().post(&url).json(&input).send().await
        })
        .await;

        match resp {
            Ok(Ok(r)) if r.status().is_success() => {
                let v: Value = r.json().await.unwrap_or_else(|_| json!({}));
                return Ok(v);
            }
            _ => {
                if attempt == retries {
                    return Err(anyhow!("tool call failed after {} attempts", retries + 1));
                }
                let jitter: f64 = rng().random_range(0.5..1.5);
                let delay = (base as f64 * 2f64.powi(attempt as i32) * jitter).round() as u64;
                sleep(Duration::from_secs(delay)).await;
            }
        }
    }
    Err(anyhow!("unreachable"))
}

fn render_json_strings(value: &mut Value, ctx: &Value) -> Result<()> {
    let h = Handlebars::new();
    match value {
        Value::String(s) => {
            *s = h.render_template(s, ctx)?;
        }
        Value::Array(arr) => {
            for v in arr {
                render_json_strings(v, ctx)?;
            }
        }
        Value::Object(map) => {
            for (_, v) in map.iter_mut() {
                render_json_strings(v, ctx)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_json_strings_substitutes_nested_templates() {
        let mut value = json!({
            "greeting": "Hello {{state.user}}",
            "items": ["{{envelope.chat_id}}", "{{state.extra}}"]
        });
        let ctx = json!({
            "state": { "user": "Alice", "extra": "item-2" },
            "envelope": { "chat_id": "chat-1" }
        });

        render_json_strings(&mut value, &ctx).unwrap();
        assert_eq!(value["greeting"], "Hello Alice");
        assert_eq!(value["items"][0], "chat-1");
        assert_eq!(value["items"][1], "item-2");
    }

    #[test]
    fn render_json_strings_leaves_non_strings() {
        let mut value = json!({
            "count": 3,
            "flags": [true, false],
            "note": "Hi {{state.name}}"
        });
        let ctx = json!({ "state": { "name": "Bob" } });

        render_json_strings(&mut value, &ctx).unwrap();
        assert_eq!(value["count"], 3);
        assert_eq!(value["flags"][0], true);
        assert_eq!(value["note"], "Hi Bob");
    }
}
