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

#[test]
fn removing_a_default_archives_data_and_survives_setup() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("runtime");
    json(&root, &["init"]);
    let data = root.join("business/chrome/keep-me");
    std::fs::write(&data, "preserve").unwrap();
    let removed = json(&root, &["profile", "remove", "business"]);
    assert_eq!(removed["removed"], true);
    let archive = Path::new(removed["archive"].as_str().unwrap());
    assert_eq!(
        std::fs::read_to_string(archive.join("chrome/keep-me")).unwrap(),
        "preserve"
    );
    assert!(!root.join("business").exists());
    json(&root, &["init"]);
    assert!(!root.join("business").exists());
    assert!(!run(&root, &["status", "business"]).status.success());
    assert!(
        !json(&root, &["profile", "list"])
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["profile"] == "business")
    );
    for client in ["claude", "codex", "antigravity"] {
        let out = run(&root, &["config", client, "--all-profiles"]);
        assert!(out.status.success());
        assert!(!String::from_utf8_lossy(&out.stdout).contains("abm-business"));
    }
    // Only an explicit new registration recreates a removed default, with fresh data.
    assert_eq!(
        json(&root, &["profile", "create", "business"])["port"],
        9223
    );
    assert!(!root.join("business/chrome/keep-me").exists());
    assert!(archive.join("chrome/keep-me").exists());
}

#[test]
fn removal_refuses_locks_and_keeps_other_named_profiles() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("runtime");
    json(&root, &["init"]);
    let named = json(&root, &["profile", "create", "studio-team"]);
    let lock = root.join("business/chrome/SingletonLock");
    std::fs::write(&lock, "do not delete").unwrap();
    assert!(
        !run(&root, &["profile", "remove", "business"])
            .status
            .success()
    );
    assert_eq!(std::fs::read_to_string(&lock).unwrap(), "do not delete");
    std::fs::remove_file(lock).unwrap();
    json(&root, &["profile", "remove", "business"]);
    assert_eq!(
        json(&root, &["profile", "list"])
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["profile"] == "studio-team")
            .unwrap()["port"],
        named["port"]
    );
    json(&root, &["profile", "remove", "studio-team"]);
    assert!(!root.join("studio-team").exists());
    assert!(
        !run(&root, &["profile", "remove", "unknown-team"])
            .status
            .success()
    );
}
