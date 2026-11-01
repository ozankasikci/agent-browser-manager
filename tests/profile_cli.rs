use serde_json::Value;
use std::{
    path::Path,
    process::{Command, Stdio},
};
const BIN: &str = env!("CARGO_BIN_EXE_abm");
fn run(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(BIN)
        .arg("--data-dir")
        .arg(root)
        .args(args)
        .output()
        .unwrap()
}
fn json(root: &Path, args: &[&str]) -> Value {
    let out = run(root, args);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}
#[test]
fn named_config_routes_all_clients_to_the_registered_profile() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("runtime");
    json(&root, &["init"]);
    let created = json(&root, &["profile", "create", "studio-team"]);
    assert_eq!(created["profile"], "studio-team");
    for client in ["claude", "codex", "antigravity"] {
        let out = run(&root, &["config", client, "--profile", "studio-team"]);
        assert!(out.status.success());
        if client == "codex" {
            let v: toml::Value = toml::from_str(std::str::from_utf8(&out.stdout).unwrap()).unwrap();
            assert_eq!(
                v["mcp_servers"]["abm-studio-team"]["args"][2].as_str(),
                Some("studio-team")
            );
        } else {
            let v: Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(v["mcpServers"]["abm-studio-team"]["args"][2], "studio-team");
        }
    }
    assert_eq!(
        json(&root, &["profile", "list"]).as_array().unwrap().len(),
        4
    );
    assert_eq!(
        json(&root, &["status", "studio-team"])[0]["state"],
        "stopped"
    );
    for name in ["../escape", "business/other", "UPPER"] {
        assert!(!run(&root, &["start", name, "--headless"]).status.success());
    }
    assert!(!root.parent().unwrap().join("escape").exists());
    assert!(!run(&root, &["status", "unknown-team"]).status.success());
    assert!(!root.join("unknown-team").exists());
}
#[test]
fn racing_profile_registration_keeps_one_name_and_one_port() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("runtime");
    json(&root, &["init"]);
    let first = Command::new(BIN)
        .arg("--data-dir")
        .arg(&root)
        .args(["profile", "create", "same-team"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let second = json(&root, &["profile", "create", "same-team"]);
    let first = first.wait_with_output().unwrap();
    assert!(first.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&first.stdout).unwrap(),
        second
    );
    let other = json(&root, &["profile", "create", "other-team"]);
    assert_ne!(other["port"], second["port"]);
    assert_eq!(
        json(&root, &["profile", "list"]).as_array().unwrap().len(),
        5
    );
}
