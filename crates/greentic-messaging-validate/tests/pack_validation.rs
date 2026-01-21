use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[test]
fn validator_emits_messaging_diagnostics() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("resolve repo root")
        .to_path_buf();
    let fixture_src = repo_root
        .join("tests")
        .join("fixtures")
        .join("minimal-messaging-pack");
    let staging_root = repo_root.join("target").join("fixture-packs");
    let staging = staging_root.join("minimal-messaging-pack");
    if staging.exists() {
        fs::remove_dir_all(&staging).expect("remove staging");
    }
    fs::create_dir_all(&staging_root).expect("create staging root");
    copy_dir_all(&fixture_src, &staging).expect("copy fixture");

    let lock_file = staging.join("pack.lock.json");
    let status = Command::new("greentic-pack")
        .args([
            "resolve",
            "--in",
            staging.to_str().expect("staging"),
            "--lock",
            lock_file.to_str().expect("lock"),
            "--offline",
        ])
        .status()
        .expect("run greentic-pack resolve");
    assert!(status.success(), "greentic-pack resolve failed");

    let pack_out = staging_root.join("minimal-messaging-pack.gtpack");
    let status = Command::new("greentic-pack")
        .args([
            "build",
            "--in",
            staging.to_str().expect("staging"),
            "--lock",
            lock_file.to_str().expect("lock"),
            "--gtpack-out",
            pack_out.to_str().expect("pack_out"),
            "--bundle",
            "none",
            "--offline",
            "--allow-oci-tags",
        ])
        .status()
        .expect("run greentic-pack build");
    assert!(status.success(), "greentic-pack build failed");

    let validator_pack = repo_root.join("dist").join("validators-messaging.gtpack");
    if !validator_pack.exists() {
        let status = Command::new("bash")
            .arg("scripts/build-validator-pack.sh")
            .current_dir(&repo_root)
            .status()
            .expect("build validator pack");
        assert!(status.success(), "build-validator-pack.sh failed");
    }

    let output = Command::new("greentic-pack")
        .args([
            "doctor",
            "--format",
            "json",
            "--validate",
            "--pack",
            pack_out.to_str().expect("pack_out"),
            "--validator-pack",
            validator_pack.to_str().expect("validator_pack"),
            "--offline",
            "--allow-oci-tags",
        ])
        .output()
        .expect("run greentic-pack doctor");
    assert!(output.status.success(), "greentic-pack doctor failed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("MSG_SECRETS_REQUIREMENTS_NOT_DISCOVERABLE"),
        "expected messaging validator warning in output: {stdout}"
    );
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let target = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_all(&path, &target)?;
        } else {
            fs::copy(&path, &target)?;
        }
    }
    Ok(())
}
