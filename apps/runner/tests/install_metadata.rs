use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
use gsm_core::{MessageEnvelope, Platform, TenantCtx};
use gsm_runner::engine::{ExecutionOptions, RunnerSink, ToolMode, run_flow};
use gsm_runner::model::{Flow, Node, TemplateNode};
use gsm_runner::template_node::hb_registry;
use gsm_session::shared_memory_store;
use serde_json::json;

struct CaptureSink {
    out: Arc<Mutex<Vec<gsm_core::OutMessage>>>,
}

#[async_trait]
impl RunnerSink for CaptureSink {
    async fn publish_out_message(&self, _subject: &str, out: &gsm_core::OutMessage) -> Result<()> {
        self.out.lock().expect("lock").push(out.clone());
        Ok(())
    }
}

#[tokio::test]
async fn runner_preserves_install_metadata() {
    let mut nodes = BTreeMap::new();
    nodes.insert(
        "start".to_string(),
        Node {
            qa: None,
            tool: None,
            template: Some(TemplateNode {
                template: "hello".into(),
            }),
            card: None,
            routes: vec!["end".into()],
        },
    );
    let flow = Flow {
        id: "flow".into(),
        title: None,
        description: None,
        kind: "qa".into(),
        r#in: "start".into(),
        nodes,
    };

    let tenant_ctx = TenantCtx::new("dev".parse().unwrap(), "acme".parse().unwrap());
    let mut context = BTreeMap::new();
    context.insert("provider_id".into(), json!("messaging.slack"));
    context.insert("install_id".into(), json!("install-a"));
    let env = MessageEnvelope {
        tenant: "acme".into(),
        platform: Platform::Slack,
        chat_id: "chat-1".into(),
        user_id: "user-1".into(),
        thread_id: None,
        msg_id: "msg-1".into(),
        text: Some("hi".into()),
        timestamp: "2024-01-01T00:00:00Z".into(),
        context,
    };

    let options = ExecutionOptions {
        tool_mode: ToolMode::Stub,
        allow_agent: false,
        tool_endpoint: "http://localhost:18081".into(),
    };
    let out = Arc::new(Mutex::new(Vec::new()));
    let sink = CaptureSink { out: out.clone() };
    let sessions = shared_memory_store();
    let hbs = hb_registry();

    let outcome = run_flow(
        "flow",
        &flow,
        &tenant_ctx,
        &env,
        &sessions,
        &hbs,
        &sink,
        &options,
        None,
    )
    .await
    .expect("run flow");

    assert_eq!(outcome.out_messages.len(), 1);
    let out_message = &outcome.out_messages[0];
    assert_eq!(
        out_message.meta.get("provider_id"),
        Some(&json!("messaging.slack"))
    );
    assert_eq!(
        out_message.meta.get("install_id"),
        Some(&json!("install-a"))
    );
}
