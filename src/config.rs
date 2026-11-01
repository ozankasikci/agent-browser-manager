use crate::{
    Profile,
    store::{self, Store},
};
use anyhow::{Result, ensure};
use clap::ValueEnum;
use serde_json::{Value, json};
use std::path::Path;

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Client {
    Claude,
    Codex,
    Antigravity,
}
impl Client {
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Antigravity => "antigravity",
        }
    }
}

pub fn render(
    client: Client,
    profiles: &[Profile],
    executable: &Path,
    store: &Store,
    agent: &str,
) -> Result<String> {
    store::validate_agent(agent)?;
    ensure!(
        executable.is_absolute(),
        "configuration requires an absolute binary path"
    );
    let command = executable
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("binary path must be UTF-8"))?;
    let data = store
        .root
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("data directory must be UTF-8"))?;
    let mut entries = serde_json::Map::new();
    let mut toml = String::new();
    for profile in profiles {
        let name = format!("abm-{}", profile.name());
        let args = vec![
            "mcp",
            "--profile",
            profile.name(),
            "--agent",
            agent,
            "--data-dir",
            data,
        ];
        match client {
            Client::Codex => {
                toml.push_str(&format!("[mcp_servers.{name}]\ncommand = {}\nargs = {}\nstartup_timeout_sec = 10\ntool_timeout_sec = 60\n\n",serde_json::to_string(command)?,serde_json::to_string(&args)?));
            }
            Client::Claude | Client::Antigravity => {
                let mut entry = json!({"command":command,"args":args});
                if matches!(client, Client::Claude) {
                    entry["type"] = json!("stdio");
                }
                entries.insert(name, entry);
            }
        }
    }
    if matches!(client, Client::Codex) {
        Ok(toml)
    } else {
        Ok(serde_json::to_string_pretty(&json!({"mcpServers":Value::Object(entries)}))? + "\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checked_in_templates_are_valid_and_cover_the_fixed_profiles() -> Result<()> {
        for text in [
            include_str!("../configs/claude.mcp.example.json"),
            include_str!("../configs/antigravity.mcp.example.json"),
        ] {
            let v: Value = serde_json::from_str(text)?;
            for profile in Profile::defaults() {
                let entry = &v["mcpServers"][format!("abm-{}", profile.name())];
                assert_eq!(entry["args"][2], profile.name());
                assert!(entry["command"].as_str().unwrap().ends_with("/bin/abm"));
            }
        }
        let v: toml::Value = toml::from_str(include_str!("../configs/codex.config.example.toml"))?;
        for profile in Profile::defaults() {
            assert_eq!(
                v["mcp_servers"][format!("abm-{}", profile.name())]["args"][2].as_str(),
                Some(profile.name())
            );
        }
        Ok(())
    }
    #[test]
    fn generated_configs_parse_and_do_not_invoke_a_runtime() -> Result<()> {
        let store = Store::new(Some("/tmp/abm data".into()))?;
        for client in [Client::Claude, Client::Codex, Client::Antigravity] {
            let text = render(
                client,
                &Profile::defaults(),
                Path::new("/tmp/agent manager/bin/abm"),
                &store,
                client.name(),
            )?;
            if matches!(client, Client::Codex) {
                let v: toml::Value = toml::from_str(&text)?;
                for profile in Profile::defaults() {
                    assert_eq!(
                        v["mcp_servers"][format!("abm-{}", profile.name())]["args"][2].as_str(),
                        Some(profile.name())
                    );
                }
            } else {
                let v: Value = serde_json::from_str(&text)?;
                assert_eq!(v["mcpServers"].as_object().unwrap().len(), 3);
                for profile in Profile::defaults() {
                    assert_eq!(
                        v["mcpServers"][format!("abm-{}", profile.name())]["args"][2],
                        profile.name()
                    );
                }
            }
            for forbidden in ["npx", "node", "chrome-devtools-mcp", "0.0.0.0"] {
                assert!(!text.contains(forbidden));
            }
        }
        Ok(())
    }
}
