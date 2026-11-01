use crate::{
    browser,
    store::{self, Store},
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    net::{Ipv4Addr, SocketAddr, TcpListener},
};

pub const FIRST_CUSTOM_PORT: u16 = 9225;
pub const LAST_CUSTOM_PORT: u16 = 9324;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    name: String,
    port: u16,
}

impl Profile {
    pub fn defaults() -> Vec<Self> {
        [
            ("personal", 9222),
            ("business", 9223),
            ("development", 9224),
        ]
        .into_iter()
        .map(|(name, port)| Self {
            name: name.into(),
            port,
        })
        .collect()
    }
    #[cfg(test)]
    pub fn personal() -> Self {
        Self::defaults().remove(0)
    }
    #[cfg(test)]
    pub fn development() -> Self {
        Self::defaults().remove(2)
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn port(&self) -> u16 {
        self.port
    }
    pub fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    schema: u32,
    custom: Vec<Profile>,
}

pub fn validate_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty() && name.len() <= 48,
        "profile name must contain 1 to 48 characters"
    );
    ensure!(
        name.as_bytes()[0].is_ascii_lowercase()
            && !name.ends_with('-')
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
        "profile name must start with a lowercase letter and contain only lowercase letters, numbers, or hyphens; it cannot end with a hyphen"
    );
    Ok(())
}

impl Store {
    fn catalog(&self) -> Result<Catalog> {
        let catalog: Catalog =
            store::read_json(&self.root.join("profiles.json"))?.unwrap_or(Catalog {
                schema: 1,
                custom: Vec::new(),
            });
        ensure!(catalog.schema == 1, "unrecognized profile registry version");
        let mut names: BTreeSet<_> = Profile::defaults().into_iter().map(|p| p.name).collect();
        let mut ports: BTreeSet<_> = Profile::defaults().into_iter().map(|p| p.port).collect();
        for profile in &catalog.custom {
            validate_name(profile.name())?;
            ensure!(
                (FIRST_CUSTOM_PORT..=LAST_CUSTOM_PORT).contains(&profile.port),
                "custom profile port is outside the managed range"
            );
            ensure!(
                names.insert(profile.name.clone()) && ports.insert(profile.port),
                "profile registry contains duplicate names or ports"
            );
        }
        Ok(catalog)
    }

    pub fn profiles(&self) -> Result<Vec<Profile>> {
        let _lock = self.root_lock("profiles.lock", false)?;
        let mut profiles = Profile::defaults();
        let mut custom = self.catalog()?.custom;
        custom.sort_by(|a, b| a.name.cmp(&b.name));
        profiles.extend(custom);
        Ok(profiles)
    }

    pub fn resolve_profile(&self, name: &str) -> Result<Profile> {
        validate_name(name)?;
        if let Some(profile) = Profile::defaults().into_iter().find(|p| p.name == name) {
            return Ok(profile);
        }
        self.profiles()?
            .into_iter()
            .find(|p| p.name == name)
            .context("unknown profile; use abm profile create NAME or abm start NAME first")
    }

    pub fn create_profile(&self, name: &str, requested_port: Option<u16>) -> Result<Profile> {
        validate_name(name)?;
        let _lock = self.root_lock("profiles.lock", true)?;
        let mut catalog = self.catalog()?;
        if let Some(profile) = Profile::defaults()
            .into_iter()
            .chain(catalog.custom.iter().cloned())
            .find(|p| p.name == name)
        {
            ensure!(
                requested_port.is_none_or(|port| port == profile.port),
                "profile already has a stable port; reassignment is not allowed"
            );
            store::check_dir(&self.profile_dir(&profile))?;
            store::check_dir(&self.chrome_dir(&profile))?;
            return Ok(profile);
        }
        if let Some(port) = requested_port {
            ensure!(
                (FIRST_CUSTOM_PORT..=LAST_CUSTOM_PORT).contains(&port),
                "custom ports must be in 9225..=9324; default ports cannot be reassigned"
            );
        }
        let candidates: Vec<_> = requested_port.map_or_else(
            || (FIRST_CUSTOM_PORT..=LAST_CUSTOM_PORT).collect(),
            |p| vec![p],
        );
        let mut selected = None;
        for port in candidates {
            if catalog.custom.iter().any(|p| p.port == port) {
                continue;
            }
            if !browser::listeners(port)?.is_empty() {
                continue;
            }
            if let Ok(guard) = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port))) {
                selected = Some((port, guard));
                break;
            }
        }
        let (port, _port_guard) = selected.context(
            "no available port for this named profile; choose an unused port in 9225..=9324",
        )?;
        let profile = Profile {
            name: name.into(),
            port,
        };
        let dir = self.profile_dir(&profile);
        match fs::symlink_metadata(&dir) {
            Ok(_) => bail!(
                "unregistered profile directory already exists; refusing to adopt or overwrite it"
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e).context("cannot inspect new profile directory"),
        }
        store::make_private_dir(&dir)?;
        let result = (|| -> Result<()> {
            store::make_private_dir(&self.chrome_dir(&profile))?;
            catalog.custom.push(profile.clone());
            store::write_json(&self.root.join("profiles.json"), &catalog)
        })();
        if result.is_err() {
            // Only remove newly created empty directories; never recurse over browser data.
            let _ = fs::remove_dir(self.chrome_dir(&profile));
            let _ = fs::remove_dir(&dir);
        }
        result?;
        Ok(profile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unsafe_names() {
        for name in [
            "",
            "../business",
            "business/other",
            ".hidden",
            "BUSINESS",
            "-team",
            "team-",
            "team_name",
            "a\nb",
            "profiles.json",
        ] {
            assert!(validate_name(name).is_err(), "{name}");
        }
        assert!(validate_name(&"a".repeat(49)).is_err());
        for name in ["studio-team", "a", "team42"] {
            assert!(validate_name(name).is_ok());
        }
    }
    #[test]
    fn named_profiles_keep_private_directories_and_stable_ports() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::new(Some(temp.path().join("runtime")))?;
        store.init()?;
        let profile = store.create_profile("studio-team", None)?;
        assert!((FIRST_CUSTOM_PORT..=LAST_CUSTOM_PORT).contains(&profile.port()));
        assert_eq!(store.create_profile("studio-team", None)?, profile);
        assert_eq!(store.resolve_profile("studio-team")?, profile);
        assert!(store.create_profile("studio-team", Some(9222)).is_err());
        assert!(
            store
                .create_profile("other-team", Some(profile.port()))
                .is_err()
        );
        assert_eq!(
            Store::new(Some(store.root.clone()))?.resolve_profile("studio-team")?,
            profile
        );
        let sentinel = store.chrome_dir(&Profile::personal()).join("keep-me");
        fs::write(&sentinel, "preserve")?;
        store.init()?;
        assert_eq!(fs::read_to_string(sentinel)?, "preserve");
        assert_eq!(store.profiles()?.len(), 4);
        store::check_dir(&store.chrome_dir(&profile))?;
        Ok(())
    }
    #[test]
    fn refuses_unknown_existing_directories_and_reserved_ports() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::new(Some(temp.path().join("runtime")))?;
        store.init()?;
        let dir = store.root.join("existing-team");
        store::make_private_dir(&dir)?;
        fs::write(dir.join("important"), "preserve")?;
        assert!(store.create_profile("existing-team", None).is_err());
        assert_eq!(fs::read_to_string(dir.join("important"))?, "preserve");
        for port in [0, 9222, 9223, 9224, 9325, u16::MAX] {
            assert!(store.create_profile("new-team", Some(port)).is_err());
        }
        Ok(())
    }
    #[test]
    fn corrupt_registry_cannot_alias_an_existing_profile() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::new(Some(temp.path().join("runtime")))?;
        store.init()?;
        for custom in [
            vec![Profile {
                name: "business".into(),
                port: 9225,
            }],
            vec![Profile {
                name: "team".into(),
                port: 9223,
            }],
            vec![Profile {
                name: "../business".into(),
                port: 9225,
            }],
        ] {
            store::write_json(
                &store.root.join("profiles.json"),
                &Catalog { schema: 1, custom },
            )?;
            assert!(store.profiles().is_err());
        }
        Ok(())
    }
}
