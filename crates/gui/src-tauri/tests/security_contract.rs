//! Audits the checked-in desktop capability and packaging boundary.

use std::{fs, path::Path};

use anyhow::{Context, Result};
use serde_json::Value;

fn read_json(path: &Path) -> Result<Value> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("desktop configuration reads: {}", path.display()))?;
    serde_json::from_str(&contents).context("desktop configuration is valid JSON")
}

#[test]
fn main_webview_has_only_explicit_local_command_permissions() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let capability = read_json(&root.join("capabilities/main.json"))?;
    let permissions = capability["permissions"]
        .as_array()
        .context("permissions are an array")?
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()
        .context("every permission is text")?;

    assert_eq!(capability["windows"], serde_json::json!(["main"]));
    assert!(capability.get("remote").is_none());
    assert_eq!(permissions.len(), 11);
    assert!(
        permissions
            .iter()
            .all(|permission| permission.starts_with("allow-"))
    );
    assert!(permissions.iter().all(|permission| {
        !permission.contains('*')
            && *permission != "allow-all"
            && !permission.ends_with(":allow-all")
            && !permission.contains("shell")
            && !permission.contains("fs")
            && !permission.contains("http")
    }));
    Ok(())
}

#[test]
fn production_webview_is_local_and_sidecars_are_a_pair() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let base = read_json(&root.join("tauri.conf.json"))?;
    let release = read_json(&root.join("tauri.release.conf.json"))?;
    let csp = base["app"]["security"]["csp"]
        .as_str()
        .context("CSP is text")?;

    assert!(!csp.contains("https:"));
    assert!(!csp.contains('*'));
    assert_eq!(base["app"]["windows"][0]["label"], "main");
    assert_eq!(
        release["bundle"]["externalBin"],
        serde_json::json!(["binaries/ffmpeg", "binaries/ffprobe"])
    );
    Ok(())
}
