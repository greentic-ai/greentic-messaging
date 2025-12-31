use gsm_core::{SendInput, SendOutput, validate_send_input};
use jsonschema::Validator;
use serde_json::Value;
use std::fs::File;
use std::path::{Path, PathBuf};

fn schemas_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../schemas/messaging/ops")
        .canonicalize()
        .expect("schema root exists")
}

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .canonicalize()
        .expect("fixtures root exists")
}

fn load_json(path: PathBuf) -> Value {
    let file = File::open(path).expect("file readable");
    serde_json::from_reader(file).expect("valid json")
}

#[test]
fn send_input_fixture_matches_schema_and_type() {
    let schema = load_json(schemas_root().join("send.input.schema.json"));
    let fixture_path = fixtures_root().join("send_input.json");
    let data = load_json(fixture_path);

    let compiled = Validator::new(&schema).expect("compile send schema");
    compiled.validate(&data).expect("fixture validates");

    let typed: SendInput = serde_json::from_value(data.clone()).expect("deserializes");
    validate_send_input(&typed).expect("passes logical validation");
}

#[test]
fn send_output_fixture_matches_schema_and_type() {
    let schema = load_json(schemas_root().join("send.output.schema.json"));
    let fixture_path = fixtures_root().join("send_output.json");
    let data = load_json(fixture_path);

    let compiled = Validator::new(&schema).expect("compile send output schema");
    compiled.validate(&data).expect("fixture validates");

    let _: SendOutput = serde_json::from_value(data).expect("deserializes");
}
