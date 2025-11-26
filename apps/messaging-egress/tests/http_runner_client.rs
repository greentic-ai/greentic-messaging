use axum::routing::post;
use axum::{Json, Router};
use gsm_core::{
    AdapterDescriptor, HttpRunnerClient, MessagingAdapterKind, OutKind, OutMessage, Platform,
    RunnerClient, make_tenant_ctx,
};
use serde_json::Value;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

// End-to-end check of HttpRunnerClient posting to a local runner endpoint.
// Skips if binding to localhost is not permitted in the current environment.
#[tokio::test]
async fn http_runner_client_posts_invocation() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("skipping http_runner_client_posts_invocation: {err}");
            return;
        }
    };

    let (tx, rx) = oneshot::channel::<Value>();
    let tx = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));

    let app = Router::new().route(
        "/invoke",
        post({
            let tx = tx.clone();
            move |Json(payload): Json<Value>| {
                let tx = tx.clone();
                async move {
                    if let Some(sender) = tx.lock().unwrap().take() {
                        let _ = sender.send(payload);
                    }
                    Json(())
                }
            }
        }),
    );

    let addr: SocketAddr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        if let Err(err) = axum::serve(listener, app.into_make_service()).await {
            eprintln!("runner mock server error: {err}");
        }
    });

    let client = HttpRunnerClient::new(format!("http://{addr}/invoke"), None).unwrap();

    let adapter = AdapterDescriptor {
        pack_id: "pack".into(),
        pack_version: "1.0.0".into(),
        name: "slack-main".into(),
        kind: MessagingAdapterKind::IngressEgress,
        component: "comp@1.0.0".into(),
        default_flow: Some("flows/messaging/slack/default.ygtc".into()),
        custom_flow: None,
        capabilities: None,
        source: None,
    };
    let out = OutMessage {
        ctx: make_tenant_ctx("dev".into(), Some("acme".into()), None),
        tenant: "acme".into(),
        platform: Platform::Slack,
        chat_id: "C123".into(),
        thread_id: None,
        kind: OutKind::Text,
        text: Some("hi".into()),
        message_card: None,
        adaptive_card: None,
        meta: Default::default(),
    };

    client.invoke_adapter(&out, &adapter).await.unwrap();

    let payload = tokio::time::timeout(std::time::Duration::from_secs(2), rx)
        .await
        .expect("runner should respond")
        .expect("payload should be sent");
    assert_eq!(payload["adapter"]["name"], "slack-main");
    assert_eq!(payload["message"]["platform"], "slack");

    server.abort();
}
