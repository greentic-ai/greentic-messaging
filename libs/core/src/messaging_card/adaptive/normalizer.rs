use anyhow::{Context, Result};
use serde_json::Value;

use crate::messaging_card::ir::{Element, Fact, InputKind, IrAction, MessageCardIr, Meta};
use crate::messaging_card::tier::Tier;

pub fn ac_to_ir(card: &Value) -> Result<MessageCardIr> {
    let root = card
        .as_object()
        .context("adaptive card must be an object")?;

    let mut ir = MessageCardIr {
        tier: Tier::Advanced,
        ..MessageCardIr::default()
    };

    if let Some(title) = root.get("title").and_then(|v| v.as_str()) {
        ir.head.title = Some(title.to_string());
    }

    if let Some(body) = root.get("body").and_then(|b| b.as_array()) {
        for element in body {
            if let Some(parsed) = normalize_body_element(element, &mut ir.meta) {
                ir.elements.push(parsed);
            }
        }
    }

    if let Some(actions) = root.get("actions").and_then(|a| a.as_array()) {
        for action in actions {
            if let Some(parsed) = normalize_action(action, &mut ir.meta)? {
                ir.actions.push(parsed);
            }
        }
    }

    Ok(ir)
}

fn normalize_body_element(value: &Value, meta: &mut Meta) -> Option<Element> {
    let obj = value.as_object()?;
    let element_type = obj.get("type")?.as_str()?;
    match element_type {
        "TextBlock" => {
            let text = obj.get("text")?.as_str()?.to_string();
            let markdown = obj.get("wrap").and_then(|v| v.as_bool()).unwrap_or(true);
            Some(Element::Text { text, markdown })
        }
        "Image" => {
            let url = obj.get("url")?.as_str()?.to_string();
            let alt = obj
                .get("altText")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            Some(Element::Image { url, alt })
        }
        "FactSet" => {
            meta.add_capability("facts");
            let facts = obj
                .get("facts")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
                .filter_map(|fact| {
                    let fact_obj = fact.as_object()?;
                    Some(Fact {
                        label: fact_obj.get("title")?.as_str()?.to_string(),
                        value: fact_obj.get("value")?.as_str()?.to_string(),
                    })
                })
                .collect::<Vec<_>>();
            Some(Element::FactSet { facts })
        }
        t if t.starts_with("Input.") => {
            meta.add_capability("inputs");
            let kind = match t {
                "Input.Text" => InputKind::Text,
                "Input.ChoiceSet" => InputKind::Choice,
                _ => InputKind::Text,
            };
            let label = obj
                .get("label")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let id = obj
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let required = obj
                .get("isRequired")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let choices = obj
                .get("choices")
                .and_then(|v| v.as_array())
                .map(|choices| {
                    choices
                        .iter()
                        .filter_map(|choice| {
                            let choice_obj = choice.as_object()?;
                            Some(crate::messaging_card::ir::InputChoice {
                                title: choice_obj.get("title")?.as_str()?.to_string(),
                                value: choice_obj.get("value")?.as_str()?.to_string(),
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            Some(Element::Input {
                label,
                kind,
                id,
                required,
                choices,
            })
        }
        _ => None,
    }
}

fn normalize_action(value: &Value, meta: &mut Meta) -> Result<Option<IrAction>> {
    let obj = match value.as_object() {
        Some(obj) => obj,
        None => return Ok(None),
    };

    let action_type = match obj.get("type").and_then(|t| t.as_str()) {
        Some(t) => t,
        None => return Ok(None),
    };

    match action_type {
        "Action.OpenUrl" => {
            let title = obj
                .get("title")
                .and_then(|v| v.as_str())
                .context("openUrl action missing title")?
                .to_string();
            let url = obj
                .get("url")
                .and_then(|v| v.as_str())
                .context("openUrl action missing url")?
                .to_string();
            Ok(Some(IrAction::OpenUrl { title, url }))
        }
        "Action.Submit" | "Action.Execute" => {
            if action_type == "Action.Execute" {
                meta.add_capability("execute");
            }
            let title = obj
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("Submit")
                .to_string();
            let data = obj.get("data").cloned().unwrap_or(Value::Null);
            Ok(Some(IrAction::Postback { title, data }))
        }
        "Action.ShowCard" => {
            meta.add_capability("showcard");
            Ok(None)
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maps_text_blocks() {
        let card = json!({
            "type": "AdaptiveCard",
            "version": "1.6",
            "body": [
                { "type": "TextBlock", "text": "Hello" }
            ]
        });

        let ir = ac_to_ir(&card).expect("normalize");
        assert_eq!(ir.elements.len(), 1);
    }
}
