use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    session: String,
    agent: String,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    pub run_id: String,
    pub tabs: BTreeMap<String, Grant>,
}

impl Registry {
    pub fn for_run(existing: Option<Self>, run_id: &str) -> Self {
        existing
            .filter(|r| r.run_id == run_id)
            .unwrap_or_else(|| Self {
                run_id: run_id.into(),
                tabs: BTreeMap::new(),
            })
    }
    pub fn claim(&mut self, target: &str, session: &str, agent: &str) -> Result<()> {
        ensure!(!self.tabs.contains_key(target), "tab already has an owner");
        ensure!(
            self.tabs.len() < 1024,
            "ownership registry is full; restart this managed profile after closing old tabs"
        );
        self.tabs.insert(
            target.into(),
            Grant {
                session: session.into(),
                agent: agent.into(),
            },
        );
        Ok(())
    }
    pub fn require(&self, target: &str, session: &str) -> Result<()> {
        ensure!(
            self.tabs.get(target).is_some_and(|g| g.session == session),
            "tab is not owned by this MCP session; open a new tab"
        );
        Ok(())
    }
    pub fn owned(&self, session: &str) -> Vec<String> {
        self.tabs
            .iter()
            .filter(|(_, g)| g.session == session)
            .map(|(id, _)| id.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn labels_do_not_grant_access_and_tabs_cannot_be_stolen() -> Result<()> {
        let mut r = Registry::for_run(None, "browser-a");
        r.claim("tab-a", "session-a", "codex")?;
        assert!(r.require("tab-a", "session-a").is_ok());
        assert!(r.require("tab-a", "session-b").is_err());
        assert!(r.claim("tab-a", "session-b", "codex").is_err());
        assert!(r.require("manual-tab", "session-a").is_err());
        assert!(r.owned("session-b").is_empty());
        let r = Registry::for_run(Some(r), "browser-b");
        assert!(r.require("tab-a", "session-a").is_err());
        Ok(())
    }
}
