// SPDX-License-Identifier: AGPL-3.0-only
fn main() {
    let bytes = include_bytes!("catalog/catalog.json");
    let v: serde_json::Value = serde_json::from_slice(bytes).expect("catalog.json parses");
    let serial = v["serial"].as_u64().expect("catalog serial");
    println!("cargo:rustc-env=SHRUG_CATALOG_SERIAL={serial}");
    println!("cargo:rerun-if-changed=catalog/catalog.json");
}
