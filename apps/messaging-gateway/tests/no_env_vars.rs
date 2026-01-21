use std::path::Path;
use std::io::ErrorKind;
use std::process::Command;

#[test]
fn runtime_avoids_env_var_reads() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .ancestors()
        .nth(2)
        .expect("repo root from manifest dir");

    let mut cmd = Command::new("rg");
    cmd.current_dir(repo_root)
        .arg("-n")
        .arg("env::var|std::env::var")
        .arg("--glob")
        .arg("!**/tests/**")
        .arg("--glob")
        .arg("!**/examples/**")
        .arg("apps/messaging-gateway/src")
        .arg("apps/runner/src")
        .arg("apps/messaging-egress/src")
        .arg("apps/ingress-common/src")
        .arg("apps/subscriptions-teams/src")
        .arg("libs/core/src")
        .arg("libs/security/src")
        .arg("libs/backpressure/src")
        .arg("libs/idempotency/src");

    let output = match cmd.output() {
        Ok(output) => output,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            let mut cmd = Command::new("grep");
            cmd.current_dir(repo_root)
                .arg("-R")
                .arg("-n")
                .arg("-E")
                .arg("env::var|std::env::var")
                .arg("--exclude-dir=tests")
                .arg("--exclude-dir=examples")
                .arg("apps/messaging-gateway/src")
                .arg("apps/runner/src")
                .arg("apps/messaging-egress/src")
                .arg("apps/ingress-common/src")
                .arg("apps/subscriptions-teams/src")
                .arg("libs/core/src")
                .arg("libs/security/src")
                .arg("libs/backpressure/src")
                .arg("libs/idempotency/src");
            cmd.output().expect("run grep fallback")
        }
        Err(err) => panic!("run rg: {err}"),
    };
    match output.status.code() {
        Some(1) => {}
        Some(0) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            panic!("env::var usage detected:\n{stdout}");
        }
        _ => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            panic!("rg failed: {stderr}");
        }
    }
}
