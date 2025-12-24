use std::path::PathBuf;

fn main() {
    println!("cargo:rustc-check-cfg=cfg(providers_bundle_present)");

    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir available"));
    let providers_root = manifest_dir.join("../../../greentic-messaging-providers");
    let wit_path = providers_root.join("components/webchat/wit/webchat");
    let bundle_enabled = std::env::var("GREENTIC_PROVIDER_BUNDLE_TESTS")
        .map(|value| !matches!(value.as_str(), "0" | "false" | "False" | "FALSE"))
        .unwrap_or(true);

    if bundle_enabled && wit_path.exists() {
        println!("cargo:rustc-cfg=providers_bundle_present");
    } else if bundle_enabled {
        println!(
            "cargo:warning=GREENTIC_PROVIDER_BUNDLE_TESTS set but WIT path missing at {}",
            wit_path.display()
        );
    } else {
        println!(
            "cargo:warning=provider bundle tests disabled (set GREENTIC_PROVIDER_BUNDLE_TESTS=1 to enable)"
        );
    }
}
