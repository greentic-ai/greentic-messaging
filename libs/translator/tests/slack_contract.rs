use gsm_core::{MessageCard, OutKind, OutMessage, Platform, make_tenant_ctx};
use gsm_testutil::{assert_matches_schema, assert_snapshot_json, load_card};
use gsm_translator::slack::to_slack_payloads;

fn base_out_message() -> OutMessage {
    OutMessage {
        ctx: make_tenant_ctx("acme".into(), Some("services".into()), None),
        tenant: "acme".into(),
        platform: Platform::Slack,
        chat_id: "C999".into(),
        thread_id: Some("1700000000.900100".into()),
        kind: OutKind::Card,
        text: None,
        message_card: None,

        adaptive_card: None,
        meta: Default::default(),
    }
}

fn load_card_fixture(name: &str) -> MessageCard {
    let path = format!("libs/cards/samples/{name}.json");
    let value = load_card!(&path);
    serde_json::from_value(value).expect("card fixture to deserialize")
}

#[test]
fn slack_card_contract() {
    const SCHEMA: &str = "libs/cards/schema/slack-blockkit.schema.json";
    let cases = ["hello", "weather", "approval", "error"];

    for case in cases {
        let mut out = base_out_message();
        out.message_card = Some(load_card_fixture(case));
        let payloads = to_slack_payloads(&out).expect("translate to slack payload");

        for payload in &payloads {
            assert_matches_schema(SCHEMA, payload).expect("slack payload to match schema");
        }

        let snapshot = format!("slack__{case}");
        insta::with_settings!({snapshot_path => "../__snapshots__"}, {
            assert_snapshot_json!(snapshot, &payloads);
        });
    }
}
