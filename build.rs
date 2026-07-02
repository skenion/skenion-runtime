use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=Cargo.toml");

    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let manifest_path = manifest_dir.join("Cargo.toml");
    let manifest = fs::read_to_string(&manifest_path).expect("read Cargo.toml");
    let range = read_skenion_metadata_value(&manifest, "supported-contracts-range")
        .expect("Cargo.toml [package.metadata.skenion] supported-contracts-range");

    println!("cargo:rustc-env=SKENION_RUNTIME_SUPPORTED_CONTRACTS_RANGE={range}");
}

fn read_skenion_metadata_value(manifest: &str, key: &str) -> Option<String> {
    let mut in_skenion_metadata = false;
    let expected_prefix = format!("{key} = ");

    for raw_line in manifest.lines() {
        let line = raw_line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_skenion_metadata = line == "[package.metadata.skenion]";
            continue;
        }
        if !in_skenion_metadata || line.starts_with('#') {
            continue;
        }
        let value = line.strip_prefix(&expected_prefix)?.trim();
        return value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .map(str::to_owned);
    }

    None
}
