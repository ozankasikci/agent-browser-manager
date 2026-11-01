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
    #[serde(default)]
    removed_defaults: BTreeSet<String>,
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
                removed_defaults: BTreeSet::new(),
            });
        ensure!(catalog.schema == 1, "unrecognized profile registry version");
        ensure!(
            catalog
                .removed_defaults
                .iter()
                .all(|name| Profile::defaults().iter().any(|p| p.name() == name)),
            "registry contains an unknown removed default"
        );
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
        let catalog = self.catalog()?;
        let mut profiles: Vec<_> = Profile::defaults()
            .into_iter()
            .filter(|p| !catalog.removed_defaults.contains(p.name()))
            .collect();
        let mut custom = catalog.custom;
        custom.sort_by(|a, b| a.name.cmp(&b.name));
        profiles.extend(custom);
        Ok(profiles)
    }

    pub fn resolve_profile(&self, name: &str) -> Result<Profile> {
        validate_name(name)?;
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
            .filter(|p| !catalog.removed_defaults.contains(p.name()))
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
        let preset = Profile::defaults().into_iter().find(|p| p.name() == name);
        if let Some(preset) = &preset {
            ensure!(
                requested_port.is_none_or(|port| port == preset.port()),
                "default profile ports cannot be reassigned"
            );
        } else if let Some(port) = requested_port {
            ensure!(
                (FIRST_CUSTOM_PORT..=LAST_CUSTOM_PORT).contains(&port),
                "custom ports must be in 9225..=9324; default ports cannot be reassigned"
            );
        }
        let candidates: Vec<_> = preset
            .as_ref()
            .map(Profile::port)
            .or(requested_port)
            .map_or_else(
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
            if preset.is_some() {
                catalog.removed_defaults.remove(name);
            } else {
                catalog.custom.push(profile.clone());
            }
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

    pub fn remove_profile(&self, name: &str) -> Result<std::path::PathBuf> {
        validate_name(name)?;
        let _registry_lock = self.root_lock("profiles.lock", true)?;
        let mut catalog = self.catalog()?;
        let profile = Profile::defaults()
            .into_iter()
            .filter(|p| !catalog.removed_defaults.contains(p.name()))
            .chain(catalog.custom.iter().cloned())
            .find(|p| p.name() == name)
            .context("unknown or already removed profile")?;
        let _lifecycle_lock = self.lock(&profile, "lifecycle.lock", true)?;
        browser::ensure_stopped_for_archive(self, &profile)?;
        let archives = self.root.join(".archive");
        store::make_private_dir(&archives)?;
        let archive = archives.join(format!("{}-{}", name, uuid::Uuid::new_v4()));
        ensure!(
            archive.symlink_metadata().is_err(),
            "archive destination already exists"
        );
        let original = self.profile_dir(&profile);
        if Profile::defaults().iter().any(|p| p.name() == name) {
            catalog.removed_defaults.insert(name.into());
        } else {
            catalog.custom.retain(|p| p.name() != name);
        }
        fs::rename(&original, &archive).context("cannot archive profile directory")?;
        if let Err(error) = store::write_json(&self.root.join("profiles.json"), &catalog) {
            fs::rename(&archive, &original).with_context(|| {
                format!(
                    "registry update failed; profile data remains at {}",
                    archive.display()
                )
            })?;
            return Err(error)
                .context("registry update failed; original profile directory restored");
        }
        Ok(archive)
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
                &Catalog {
                    schema: 1,
                    custom,
                    removed_defaults: BTreeSet::new(),
                },
            )?;
            assert!(store.profiles().is_err());
        }
        Ok(())
    }
}
