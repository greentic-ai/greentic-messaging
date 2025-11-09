use gsm_core::{MessageCard, OutKind, OutMessage, Platform, make_tenant_ctx};
use gsm_testutil::{assert_matches_schema, assert_snapshot_json, load_card};
use gsm_translator::teams::to_teams_adaptive;

fn base_out_message() -> OutMessage {
    OutMessage {
        ctx: make_tenant_ctx("acme".into(), Some("services".into()), None),
        tenant: "acme".into(),
        platform: Platform::Teams,
        chat_id: "19:abc123".into(),
        thread_id: None,
        kind: OutKind::Card,
        text: None,
        message_card: None,

        adaptive_card: None,
        meta: Default::default(),
    }
}

fn load_card_fixture(name: &str) -> MessageCard {
    let path = format!("../cards/samples/{name}.json");
    let value = load_card!(&path);
    serde_json::from_value(value).expect("card fixture to deserialize")
}

#[test]
fn adaptive_card_contract() {
    const SCHEMA: &str = "../cards/schema/adaptive-card.schema.json";
    let cases = ["hello", "weather", "approval", "error"];

    for case in cases {
        let mut out = base_out_message();
        out.message_card = Some(load_card_fixture(case));
        let payload = to_teams_adaptive(out.message_card.as_ref().unwrap(), &out)
            .expect("translate to adaptive card");

        assert_matches_schema(SCHEMA, &payload).expect("adaptive payload to match schema");

        let snapshot = format!("adaptive__{case}");
        insta::with_settings!({snapshot_path => "../__snapshots__"}, {
            assert_snapshot_json!(snapshot, &payload);
        });
    }
}
