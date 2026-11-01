use anyhow::{Context, Result, bail, ensure};
use fs2::FileExt;
use serde::{Serialize, de::DeserializeOwned};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

const MARKER: &[u8] = b"agent-browser-manager-v1\n";
const MAX_STATE: u64 = 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Store {
    pub root: PathBuf,
}

pub struct Lock {
    _file: File,
}

impl Store {
    pub fn new(root: Option<PathBuf>) -> Result<Self> {
        let root = match root {
            Some(root) => root,
            None => {
                PathBuf::from(std::env::var_os("HOME").context("HOME is unset; use --data-dir")?)
                    .join("Library/Application Support/Agent Browser Manager")
            }
        };
        ensure!(
            root.is_absolute(),
            "data directory must be an absolute path"
        );
        ensure!(
            !root
                .components()
                .any(|p| matches!(p, std::path::Component::ParentDir)),
            "data directory cannot contain .."
        );
        Ok(Self { root })
    }

    pub fn init(&self) -> Result<()> {
        let created = match fs::DirBuilder::new().mode(0o700).create(&self.root) {
            Ok(()) => true,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
            Err(e) => {
                return Err(e)
                    .context("cannot create private data directory; ensure its parent exists");
            }
        };
        check_dir(&self.root)?;
        let marker = self.root.join(".abm-root");
        if !marker.try_exists()? {
            ensure!(
                created || fs::read_dir(&self.root)?.next().is_none(),
                "refusing to initialize a populated, unrecognized directory"
            );
            let mut f = private_open(&marker, true)?;
            f.write_all(MARKER)?;
            f.sync_all()?;
        }
        self.check()?;
        for profile in crate::Profile::ALL {
            let dir = self.profile_dir(profile);
            make_private_dir(&dir)?;
            make_private_dir(&dir.join("chrome"))?;
        }
        Ok(())
    }

    pub fn check(&self) -> Result<()> {
        check_dir(&self.root).context("run abm init to create the private data directory")?;
        let mut data = Vec::new();
        private_read(&self.root.join(".abm-root"))?
            .take(128)
            .read_to_end(&mut data)?;
        ensure!(data == MARKER, "unrecognized data directory marker");
        Ok(())
    }

    pub fn profile_dir(&self, p: crate::Profile) -> PathBuf {
        self.root.join(p.name())
    }
    pub fn chrome_dir(&self, p: crate::Profile) -> PathBuf {
        self.profile_dir(p).join("chrome")
    }
    pub fn path(&self, p: crate::Profile, name: &str) -> PathBuf {
        self.profile_dir(p).join(name)
    }

    pub fn lock(&self, p: crate::Profile, name: &str, exclusive: bool) -> Result<Lock> {
        self.check()?;
        check_dir(&self.profile_dir(p))?;
        check_dir(&self.chrome_dir(p))?;
        let file = private_open(&self.path(p, name), false)?;
        if exclusive {
            FileExt::lock_exclusive(&file)?;
        } else {
            FileExt::lock_shared(&file)?;
        }
        Ok(Lock { _file: file })
    }
}

pub fn make_private_dir(path: &Path) -> Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e.into()),
    }
    check_dir(path)
}

fn check_owner_mode(meta: &fs::Metadata) -> Result<()> {
    // SAFETY: geteuid has no arguments or failure mode.
    ensure!(
        meta.uid() == unsafe { libc::geteuid() },
        "runtime path belongs to a different user"
    );
    ensure!(
        meta.mode() & 0o077 == 0,
        "runtime permissions are too broad; require private directories (700) and files (600)"
    );
    Ok(())
}

pub fn check_dir(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    ensure!(
        meta.is_dir() && !meta.file_type().is_symlink(),
        "runtime directory must be a real directory, not a symlink"
    );
    check_owner_mode(&meta)
}

fn private_open(path: &Path, create_new: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    if create_new {
        options.create_new(true);
    } else {
        options.create(true).truncate(false);
    }
    let file = options.open(path)?;
    check_file(&file)?;
    Ok(file)
}

fn check_file(file: &File) -> Result<()> {
    let meta = file.metadata()?;
    ensure!(
        meta.is_file() && meta.nlink() == 1,
        "runtime file must be a regular file without hard links"
    );
    check_owner_mode(&meta)
}

fn private_read(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    check_file(&file)?;
    Ok(file)
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    let file = match private_read(path) {
        Ok(file) => file,
        Err(e)
            if e.downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(None);
        }
        Err(e) => return Err(e).context("cannot safely read runtime state"),
    };
    let mut data = Vec::new();
    file.take(MAX_STATE + 1).read_to_end(&mut data)?;
    ensure!(data.len() as u64 <= MAX_STATE, "runtime state is too large");
    Ok(Some(
        serde_json::from_slice(&data).context("invalid runtime state")?,
    ))
}

pub fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        ensure!(
            meta.is_file() && meta.nlink() == 1,
            "refusing to replace a symlink or hard-linked state file"
        );
        check_owner_mode(&meta)?;
    }
    let mut file =
        tempfile::NamedTempFile::new_in(path.parent().context("state file has no parent")?)?;
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")?;
    file.as_file().sync_all()?;
    file.persist(path)
        .map_err(|_| anyhow::anyhow!("cannot atomically replace runtime state"))?;
    File::open(path.parent().unwrap())?.sync_all()?;
    Ok(())
}

pub fn validate_agent(agent: &str) -> Result<()> {
    if agent.is_empty()
        || agent.len() > 64
        || !agent
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        bail!("agent label must be 1 to 64 ASCII letters, numbers, dots, hyphens or underscores");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    #[test]
    fn private_and_idempotent() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let store = Store::new(Some(tmp.path().join("state")))?;
        store.init()?;
        store.init()?;
        assert_eq!(fs::metadata(&store.root)?.mode() & 0o777, 0o700);
        for p in crate::Profile::ALL {
            assert_eq!(fs::metadata(store.chrome_dir(p))?.mode() & 0o777, 0o700);
        }
        let path = store.path(crate::Profile::Personal, "test.json");
        write_json(&path, &vec![1, 2])?;
        assert_eq!(fs::metadata(&path)?.mode() & 0o777, 0o600);
        assert_eq!(read_json::<Vec<i32>>(&path)?, Some(vec![1, 2]));
        Ok(())
    }
    #[test]
    fn rejects_populated_symlink_and_loose_permissions() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let existing = tmp.path().join("existing");
        make_private_dir(&existing)?;
        fs::write(existing.join("important"), "preserve")?;
        assert!(Store::new(Some(existing.clone()))?.init().is_err());
        let link = tmp.path().join("link");
        symlink(&existing, &link)?;
        assert!(Store::new(Some(link))?.init().is_err());
        fs::set_permissions(&existing, fs::Permissions::from_mode(0o755))?;
        assert!(Store::new(Some(existing))?.init().is_err());
        Ok(())
    }
    #[test]
    fn refuses_symlink_or_hardlinked_state() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let store = Store::new(Some(tmp.path().join("state")))?;
        store.init()?;
        let original = store.root.join("a");
        write_json(&original, &1)?;
        let link = store.root.join("b");
        symlink(&original, &link)?;
        assert!(read_json::<u32>(&link).is_err());
        assert!(write_json(&link, &2).is_err());
        fs::remove_file(&link)?;
        fs::hard_link(&original, &link)?;
        assert!(read_json::<u32>(&original).is_err());
        assert!(write_json(&link, &2).is_err());
        Ok(())
    }
}
