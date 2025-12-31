use std::process::Command;

use tempfile::NamedTempFile;

const DRY_ENV: &str = "GREENTIC_MESSAGING_CLI_DRY_RUN";

fn cli_cmd() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_greentic-messaging"));
    cmd.env(DRY_ENV, "1");
    cmd
}

fn run_and_capture(args: &[&str]) -> String {
    let mut cmd = cli_cmd();
    cmd.args(args);
    let output = cmd.output().expect("run greentic-messaging CLI");
    if !output.status.success() {
        panic!(
            "CLI command {:?} failed: status={:?}\nstdout={}\nstderr={}",
            args,
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn serve_ingress_slack_dry_run() {
    let stdout = run_and_capture(&["serve", "ingress", "slack", "--tenant", "acme"]);
    assert!(
        stdout.contains("cargo run -p gsm-gateway"),
        "stdout did not contain dry-run marker:\n{}",
        stdout
    );
}

#[test]
fn serve_ingress_with_pack_sets_env() {
    let tmp = NamedTempFile::new().unwrap();
    let stdout = run_and_capture(&[
        "serve",
        "ingress",
        "webchat",
        "--tenant",
        "acme",
        "--pack",
        tmp.path().to_str().unwrap(),
    ]);
    assert!(
        stdout.contains("MESSAGING_ADAPTER_PACK_PATHS"),
        "stdout did not include pack env:\n{}",
        stdout
    );
    assert!(
        stdout.contains("cargo run -p gsm-gateway"),
        "stdout did not contain gateway run:\n{}",
        stdout
    );
}

#[test]
fn flows_run_dry_run() {
    let tmp = NamedTempFile::new().unwrap();
    let stdout = run_and_capture(&[
        "flows",
        "run",
        "--flow",
        tmp.path().to_str().unwrap(),
        "--platform",
        "slack",
        "--tenant",
        "acme",
    ]);
    assert!(
        stdout.contains("dry-run) make run-runner"),
        "stdout did not contain dry-run marker:\n{}",
        stdout
    );
}

#[test]
fn messaging_test_wrapper_dry_run() {
    let stdout = run_and_capture(&["test", "fixtures"]);
    assert!(
        stdout.contains("dry-run) cargo run -p greentic-messaging-test"),
        "stdout did not contain dry-run marker:\n{}",
        stdout
    );
}

#[test]
fn dev_down_dry_run() {
    let stdout = run_and_capture(&["dev", "down"]);
    assert!(
        stdout.contains("dry-run) make stack-down"),
        "stdout did not contain stack-down marker:\n{}",
        stdout
    );
}

#[test]
fn info_lists_adapters_from_pack() {
    let pack = NamedTempFile::new().unwrap();
    std::fs::write(
        pack.path(),
        r#"
id: info-pack
version: 0.0.1
messaging:
  adapters:
    - name: info-ingress
      kind: ingress
      component: info@0.0.1
    - name: info-egress
      kind: egress
      component: info@0.0.1
    - name: info-both
      kind: ingress-egress
      component: info@0.0.1
"#,
    )
    .unwrap();

    let stdout = run_and_capture(&[
        "info",
        "--pack",
        pack.path().to_str().unwrap(),
        "--no-default-packs",
    ]);
    assert!(
        stdout.contains("info-ingress")
            && stdout.contains("info-egress")
            && stdout.contains("info-both"),
        "stdout did not list adapters:\n{}",
        stdout
    );
}

#[test]
fn admin_wrappers_dry_run() {
    let slack = run_and_capture(&[
        "admin",
        "slack",
        "oauth-helper",
        "--",
        "--listen",
        "0.0.0.0:8085",
    ]);
    assert!(
        slack.contains("dry-run) cargo run -p gsm-slack-oauth"),
        "stdout did not contain dry-run marker:\n{}",
        slack
    );

    let teams = run_and_capture(&[
        "admin",
        "teams",
        "setup",
        "--",
        "--tenant",
        "t",
        "--client-id",
        "c",
        "--client-secret",
        "s",
        "--chat-id",
        "chat",
    ]);
    assert!(
        teams.contains("dry-run) cargo run --manifest-path scripts/Cargo.toml --bin teams_setup"),
        "stdout did not contain teams setup marker:\n{}",
        teams
    );

    let telegram = run_and_capture(&["admin", "telegram", "setup"]);
    assert!(
        telegram
            .contains("dry-run) cargo run --manifest-path scripts/Cargo.toml --bin telegram_setup"),
        "stdout did not contain telegram setup marker:\n{}",
        telegram
    );

    let whatsapp = run_and_capture(&["admin", "whatsapp", "setup"]);
    assert!(
        whatsapp
            .contains("dry-run) cargo run --manifest-path scripts/Cargo.toml --bin whatsapp_setup"),
        "stdout did not contain whatsapp setup marker:\n{}",
        whatsapp
    );
}
