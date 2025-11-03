use anyhow::Result;
use gsm_core::MessageEnvelope;
use handlebars::Handlebars;
use serde_json::{Value, json};

pub fn hb_registry() -> Handlebars<'static> {
    let mut h = Handlebars::new();
    h.set_strict_mode(true);
    h
}

pub fn render_template(
    tpl: &crate::model::TemplateNode,
    hbs: &Handlebars<'static>,
    env: &MessageEnvelope,
    state: &Value,
    payload: &Value,
) -> Result<String> {
    let ctx = json!({
      "envelope": env,
      "state": state,
      "payload": payload
    });
    Ok(hbs.render_template(&tpl.template, &ctx)?)
}
