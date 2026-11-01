//! Explicit CLI-only migration. Authentication state is never exposed as an MCP tool.
use crate::{
    Profile, browser,
    profile::validate_name,
    store::{self, Store},
};
use age::secrecy::SecretString;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{self, BufReader, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
};
use zeroize::Zeroizing;

pub const DEFAULT_MAX_BYTES: u64 = 20 * 1024 * 1024 * 1024;
const MAX_MANIFEST: u64 = 16 * 1024 * 1024;
const MAX_ENTRIES: usize = 200_000;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CookieStore {
    directory: String,
    cookies: Vec<Value>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: String,
    version: u32,
    platform: String,
    profile: String,
    cookie_stores: Vec<CookieStore>,
    skipped_opaque_cookies: usize,
}

pub fn passphrase(file: Option<&Path>, confirm: bool) -> Result<SecretString> {
    let mut value = if let Some(path) = file {
        let f = secure_file(path)?;
        ensure!(
            f.metadata()?.mode() & 0o7777 == 0o600,
            "passphrase file must be private (600)"
        );
        let mut bytes = Zeroizing::new(Vec::new());
        f.take(4097).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 4096, "passphrase file is too large");
        let text =
            std::str::from_utf8(&bytes).context("passphrase file must contain UTF-8 text")?;
        Zeroizing::new(text.trim_end_matches(['\r', '\n']).to_owned())
    } else {
        let first = Zeroizing::new(
            rpassword::prompt_password("Archive passphrase: ")
                .context("cannot read passphrase from terminal; use a private --passphrase-file")?,
        );
        if confirm {
            let second = Zeroizing::new(rpassword::prompt_password("Confirm passphrase: ")?);
            ensure!(*first == *second, "passphrases do not match");
        }
        first
    };
    ensure!(
        (12..=1024).contains(&value.len()),
        "passphrase must contain 12 to 1024 bytes"
    );
    Ok(SecretString::from(std::mem::take(&mut *value)))
}

fn secure_file(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .context("cannot safely open transfer input")?;
    let m = file.metadata()?;
    // SAFETY: geteuid has no arguments or failure mode.
    ensure!(
        m.is_file() && m.nlink() == 1 && m.uid() == unsafe { libc::geteuid() },
        "transfer input must be a regular file owned by you, without links"
    );
    Ok(file)
}

fn checked_path(path: &Path) -> Result<()> {
    ensure!(
        path.to_str().is_some_and(|s| !s.contains('\\')
            && !s.chars().any(char::is_control)
            && s.len() <= 4096),
        "unsupported archive path"
    );
    ensure!(
        !path.as_os_str().is_empty()
            && path.components().count() <= 64
            && path.components().all(|c| matches!(c, Component::Normal(_))),
        "unsafe archive path"
    );
    Ok(())
}
fn check_directory_name(name: &str) -> Result<()> {
    checked_path(Path::new(name))?;
    ensure!(
        Path::new(name).components().count() == 1
            && !matches!(name, "System Profile" | "Guest Profile"),
        "unsupported Chrome profile directory"
    );
    Ok(())
}
fn transient(path: &Path) -> bool {
    path.components().count() == 1
        && path.file_name().and_then(|s| s.to_str()).is_some_and(|s| {
            s.starts_with("Singleton")
                || matches!(
                    s,
                    "DevToolsActivePort" | "RunningChromeVersion" | "lockfile"
                )
        })
}

fn internal_profiles(chrome: &Path) -> Result<Vec<String>> {
    let mut names = BTreeSet::new();
    let local = chrome.join("Local State");
    if local.try_exists()? {
        let file = secure_file(&local)?;
        let mut data = Zeroizing::new(Vec::new());
        file.take(MAX_MANIFEST + 1).read_to_end(&mut data)?;
        ensure!(
            data.len() as u64 <= MAX_MANIFEST,
            "Chrome local state is too large"
        );
        let v: Value = serde_json::from_slice(&data)
            .map_err(|_| anyhow::anyhow!("invalid Chrome local state"))?;
        if let Some(profiles) = v["profile"]["info_cache"].as_object() {
            for name in profiles.keys() {
                if matches!(name.as_str(), "System Profile" | "Guest Profile") {
                    continue;
                }
                check_directory_name(name)?;
                if chrome.join(name).join("Preferences").is_file() {
                    names.insert(name.clone());
                }
            }
        }
    }
    if names.is_empty() {
        names.insert("Default".into());
    }
    for name in &names {
        let dir = chrome.join(name);
        if dir.exists() {
            store::check_dir(&dir)?;
        }
    }
    Ok(names.into_iter().collect())
}

fn portable_cookie(cookie: &Value) -> Result<Option<Value>> {
    ensure!(cookie.is_object(), "invalid cookie data in transfer");
    if cookie["partitionKeyOpaque"] == true {
        return Ok(None);
    }
    let mut out = serde_json::Map::new();
    for field in ["name", "value", "domain", "path"] {
        ensure!(cookie[field].is_string(), "invalid cookie data in transfer");
        out.insert(field.into(), cookie[field].clone());
    }
    for field in [
        "secure",
        "httpOnly",
        "sameSite",
        "priority",
        "sourceScheme",
        "sourcePort",
        "partitionKey",
    ] {
        if let Some(value) = cookie.get(field) {
            out.insert(field.into(), value.clone());
        }
    }
    if cookie["session"] != true
        && cookie["expires"]
            .as_f64()
            .is_some_and(|n| n > 0. && n.is_finite())
    {
        out.insert("expires".into(), cookie["expires"].clone());
    }
    Ok(Some(Value::Object(out)))
}

fn collect_cookies(
    store: &Store,
    profile: &Profile,
    chrome: Option<&Path>,
    stop_running: bool,
) -> Result<(Vec<CookieStore>, usize)> {
    let mut snapshots = Vec::new();
    let mut skipped = 0;
    if browser::ensure_stopped_for_archive(store, profile).is_err() {
        ensure!(
            stop_running,
            "profile is running; use --stop to capture live cookies and close it, or stop it yourself first"
        );
        let (_, mut cdp) = browser::connect(store, profile)?;
        let captured = (|| -> Result<CookieStore> {
            let directory = active_directory(&mut cdp, &store.chrome_dir(profile))?;
            Ok(CookieStore {
                directory,
                cookies: read_cookies(&mut cdp, &mut skipped)?,
            })
        })();
        browser::stop_locked(store, profile)
            .context("Chrome did not stop after live capture; no archive was published")?;
        snapshots.push(captured.context("live cookie capture failed; no archive was published")?);
    }
    for directory in internal_profiles(&store.chrome_dir(profile))? {
        if snapshots.iter().any(|s| s.directory == directory) {
            continue;
        }
        browser::start_locked(store, profile, chrome, true, Some(&directory))?;
        let captured = (|| -> Result<Vec<Value>> {
            let (_, mut cdp) = browser::connect(store, profile)?;
            read_cookies(&mut cdp, &mut skipped)
        })();
        let stopped = browser::stop_locked(store, profile);
        stopped.context("Chrome did not stop after cookie capture; no archive was published")?;
        let cookies = captured.context("cookie capture failed; no archive was published")?;
        snapshots.push(CookieStore { directory, cookies });
    }
    Ok((snapshots, skipped))
}

fn read_cookies(cdp: &mut crate::cdp::Cdp, skipped: &mut usize) -> Result<Vec<Value>> {
    let response = cdp.call("Storage.getCookies", json!({}), None)?;
    let all = response["cookies"]
        .as_array()
        .context("Chrome did not return cookie state")?;
    let mut cookies = Vec::new();
    for cookie in all {
        match portable_cookie(cookie)? {
            Some(c) => cookies.push(c),
            None => *skipped += 1,
        }
    }
    Ok(cookies)
}

fn active_directory(cdp: &mut crate::cdp::Cdp, root: &Path) -> Result<String> {
    let opened = cdp.call(
        "Target.createTarget",
        json!({"url":"chrome://version/","background":true}),
        None,
    )?;
    let target = opened["targetId"]
        .as_str()
        .context("cannot identify active Chrome profile")?;
    let result = (|| -> Result<String> {
        let session = cdp.attach(target)?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            // Navigation can replace the document between these calls. Retry
            // transient/stale DOM reads, without trusting any unverified path.
            let text = (|| -> Result<String> {
                let doc = cdp.call("DOM.getDocument", json!({"depth":0}), Some(&session))?;
                let selected = cdp.call(
                    "DOM.querySelector",
                    json!({"nodeId":doc["root"]["nodeId"],"selector":"#profile_path"}),
                    Some(&session),
                )?;
                ensure!(
                    selected["nodeId"].as_u64().is_some_and(|id| id > 0),
                    "Chrome profile identity is still loading"
                );
                let node = cdp.call(
                    "DOM.describeNode",
                    json!({"nodeId":selected["nodeId"],"depth":1}),
                    Some(&session),
                )?;
                Ok(node["node"]["children"]
                    .as_array()
                    .context("cannot read Chrome profile identity")?
                    .iter()
                    .filter_map(|n| n["nodeValue"].as_str())
                    .collect())
            })();
            if let Ok(text) = text {
                // WebUI creates the element before asynchronously populating its text.
                if !text.trim().is_empty() {
                    let actual = fs::canonicalize(Path::new(text.trim()))
                        .context("cannot verify Chrome profile identity")?;
                    let root = fs::canonicalize(root)?;
                    let directory = actual
                        .strip_prefix(&root)
                        .context("Chrome reported a profile outside its managed directory")?
                        .to_str()
                        .context("unsupported Chrome directory name")?
                        .to_owned();
                    check_directory_name(&directory)?;
                    return Ok(directory);
                }
            }
            ensure!(
                std::time::Instant::now() < deadline,
                "cannot identify active Chrome profile in time"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    })();
    let _ = cdp.call("Target.closeTarget", json!({"targetId":target}), None);
    result
}

fn check_git_output(parent: &Path, output: &Path) -> Result<()> {
    if parent
        .ancestors()
        .any(|dir| dir.join(".git").symlink_metadata().is_ok())
    {
        // The directory must be ignored too: encryption writes a temporary
        // file there before publishing the final archive. Do not use --no-index,
        // since a previously tracked archive must still be refused.
        for path in [parent, output] {
            let status = Command::new("git")
                .arg("-C")
                .arg(parent)
                .args([
                    "-c",
                    "core.fsmonitor=false",
                    "check-ignore",
                    "--quiet",
                    "--",
                ])
                .arg(path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .context("cannot verify Git ignore rules for the export destination")?;
            ensure!(
                status.success(),
                "exports inside Git repositories require a gitignored directory and an untracked, ignored archive; use the project's exports/ folder"
            );
        }
    }
    Ok(())
}

pub fn export(
    store: &Store,
    name: &str,
    output: &Path,
    password: SecretString,
    max_bytes: u64,
    chrome: Option<&Path>,
    stop_running: bool,
) -> Result<Value> {
    ensure!(max_bytes > 0, "maximum transfer size must be positive");
    let profile = store.resolve_profile(name)?;
    ensure!(output.is_absolute(), "output must be an absolute path");
    let parent = fs::canonicalize(output.parent().context("output has no parent")?)?;
    let output = parent.join(output.file_name().context("output has no filename")?);
    check_git_output(&parent, &output)?;
    ensure!(
        output.symlink_metadata().is_err(),
        "output already exists; refusing to overwrite it"
    );
    ensure!(
        !output.starts_with(fs::canonicalize(&store.root)?),
        "save transfer archives outside the manager's runtime directory"
    );
    let _lock = store.lock(&profile, "lifecycle.lock", true)?;
    let (cookie_stores, skipped) = collect_cookies(store, &profile, chrome, stop_running)?;
    browser::ensure_stopped_for_archive(store, &profile)?;
    if cookie_stores.iter().any(|snapshot| {
        snapshot
            .cookies
            .iter()
            .any(|cookie| cookie.get("expires").is_none())
    }) {
        // Closing Chrome for a snapshot must not discard captured session
        // cookies on the source's next manager-controlled launch either.
        store::write_json(
            &store.path(&profile, "transfer.json"),
            &json!({"schema":1,"restore_last_session":true}),
        )?;
    }
    let count: usize = cookie_stores.iter().map(|s| s.cookies.len()).sum();
    let manifest = Manifest {
        format: "agent-browser-manager".into(),
        version: 1,
        platform: std::env::consts::OS.into(),
        profile: name.into(),
        cookie_stores,
        skipped_opaque_cookies: skipped,
    };
    let mut encrypted = tempfile::NamedTempFile::new_in(&parent)?;
    write_archive(
        encrypted.as_file_mut(),
        password,
        &store.chrome_dir(&profile),
        &manifest,
        max_bytes,
    )?;
    browser::ensure_stopped_for_archive(store, &profile)
        .context("profile was opened during export; no archive was published")?;
    encrypted.as_file().sync_all()?;
    check_git_output(&parent, &output)?;
    encrypted.persist_noclobber(&output).map_err(|_| {
        anyhow::anyhow!("could not publish encrypted archive without overwriting a file")
    })?;
    File::open(&parent)?.sync_all()?;
    Ok(
        json!({"profile":name,"exported":true,"archive":output,"portable_cookies":count,"cookie_profiles":manifest.cookie_stores.len(),"skipped_opaque_cookies":skipped,"profile_state":"stopped","note":"Device-bound sessions and OS-protected passwords may require signing in again."}),
    )
}

fn encryptor(password: SecretString) -> Result<age::Encryptor> {
    let mut recipient = age::scrypt::Recipient::new(password);
    // Fixed interoperable work factor, bounded to 256 MiB. Unit tests use synthetic data.
    recipient.set_work_factor(if cfg!(test) { 10 } else { 18 });
    age::Encryptor::with_recipients(std::iter::once(&recipient as &dyn age::Recipient))
        .context("cannot initialize archive encryption")
}
fn append_bytes<W: Write>(tar: &mut tar::Builder<W>, path: &str, bytes: &[u8]) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o600);
    header.set_cksum();
    tar.append_data(&mut header, path, bytes)?;
    Ok(())
}
fn write_archive(
    output: impl Write,
    password: SecretString,
    chrome: &Path,
    manifest: &Manifest,
    max_bytes: u64,
) -> Result<()> {
    let encrypted = encryptor(password)?
        .wrap_output(output)
        .context("cannot write encrypted archive")?;
    let mut tar = tar::Builder::new(encrypted);
    let data = Zeroizing::new(
        serde_json::to_vec(manifest)
            .map_err(|_| anyhow::anyhow!("cannot encode transfer manifest"))?,
    );
    ensure!(
        data.len() as u64 <= MAX_MANIFEST,
        "transfer manifest is too large"
    );
    append_bytes(&mut tar, "manifest.json", &data)?;
    let mut budget = Budget {
        bytes: max_bytes,
        entries: MAX_ENTRIES,
    };
    append_tree(&mut tar, chrome, Path::new(""), &mut budget)?;
    tar.into_inner()?
        .finish()
        .context("cannot finish authenticated archive")?;
    Ok(())
}
struct Budget {
    bytes: u64,
    entries: usize,
}
impl Budget {
    fn consume(&mut self, size: u64) -> Result<()> {
        ensure!(
            self.entries > 0 && size <= self.bytes,
            "archive exceeds its entry or data-size limit"
        );
        self.entries -= 1;
        self.bytes -= size;
        Ok(())
    }
}
fn append_tree<W: Write>(
    tar: &mut tar::Builder<W>,
    root: &Path,
    relative: &Path,
    budget: &mut Budget,
) -> Result<()> {
    if transient(relative) {
        return Ok(());
    }
    let source = root.join(relative);
    let meta = fs::symlink_metadata(&source)?;
    let archive_path = Path::new("chrome").join(relative);
    checked_path(&archive_path)?;
    if archive_path.as_os_str().len() > 100 {
        budget.consume(0)?;
    }
    ensure!(
        !meta.file_type().is_symlink(),
        "profile contains a non-portable symlink; no archive was published"
    );
    if meta.is_dir() {
        budget.consume(0)?;
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Directory);
        header.set_size(0);
        header.set_mode(0o700);
        header.set_cksum();
        tar.append_data(&mut header, &archive_path, io::empty())?;
        let mut children: Vec<_> = fs::read_dir(&source)?.collect::<std::io::Result<_>>()?;
        children.sort_by_key(|c| c.file_name());
        for child in children {
            append_tree(tar, root, &relative.join(child.file_name()), budget)?;
        }
    } else {
        ensure!(
            meta.is_file() && meta.nlink() == 1,
            "profile contains an unsupported linked or special file"
        );
        budget.consume(meta.len())?;
        let mut file = secure_file(&source)?;
        let before = file.metadata()?;
        ensure!(
            before.ino() == meta.ino() && before.dev() == meta.dev(),
            "profile changed during export"
        );
        let mut header = tar::Header::new_gnu();
        header.set_size(before.len());
        header.set_mode(0o600);
        header.set_cksum();
        tar.append_data(&mut header, &archive_path, (&mut file).take(before.len()))?;
        let after = file.metadata()?;
        ensure!(
            after.len() == before.len()
                && after.mtime() == before.mtime()
                && after.mtime_nsec() == before.mtime_nsec(),
            "profile changed during export"
        );
    }
    Ok(())
}

fn validate_manifest(manifest: &Manifest) -> Result<()> {
    ensure!(
        manifest.format == "agent-browser-manager" && manifest.version == 1,
        "unsupported transfer archive version"
    );
    ensure!(
        manifest.platform == std::env::consts::OS,
        "cross-platform profile imports are not supported; use another Mac"
    );
    validate_name(&manifest.profile)?;
    ensure!(
        !manifest.cookie_stores.is_empty() && manifest.cookie_stores.len() <= 100,
        "invalid cookie profile list"
    );
    let mut names = BTreeSet::new();
    for snapshot in &manifest.cookie_stores {
        check_directory_name(&snapshot.directory)?;
        ensure!(
            names.insert(&snapshot.directory),
            "duplicate cookie profile directory"
        );
        ensure!(
            snapshot.cookies.len() <= 100_000,
            "too many cookies in transfer"
        );
        for cookie in &snapshot.cookies {
            ensure!(
                portable_cookie(cookie)?.is_some(),
                "non-portable cookie in transfer manifest"
            );
        }
    }
    Ok(())
}

fn read_archive(
    input: impl io::BufRead,
    password: SecretString,
    stage: &Path,
    max_bytes: u64,
) -> Result<Manifest> {
    let decryptor = age::Decryptor::new_buffered(input)
        .map_err(|_| anyhow::anyhow!("invalid encrypted transfer archive"))?;
    ensure!(
        decryptor.is_scrypt(),
        "transfer archive must be passphrase encrypted"
    );
    let mut identity = age::scrypt::Identity::new(password);
    identity.set_max_work_factor(18);
    let reader = decryptor
        .decrypt(std::iter::once(&identity as &dyn age::Identity))
        .map_err(|_| {
            anyhow::anyhow!("could not decrypt archive; wrong passphrase or damaged file")
        })?;
    extract_archive(reader, stage, max_bytes)
}
fn extract_archive(reader: impl Read, stage: &Path, max_bytes: u64) -> Result<Manifest> {
    let mut archive = tar::Archive::new(reader);
    let mut manifest = None;
    let mut long_name: Option<PathBuf> = None;
    let mut seen = BTreeSet::new();
    let mut budget = Budget {
        bytes: max_bytes,
        entries: MAX_ENTRIES,
    };
    for entry in archive
        .entries()
        .context("invalid transfer archive")?
        .raw(true)
    {
        let mut entry = entry.context("invalid transfer entry")?;
        if entry.header().entry_type().is_gnu_longname() {
            ensure!(
                long_name.is_none() && entry.size() <= 4097,
                "invalid long archive path"
            );
            budget.consume(0)?;
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            if bytes.last() == Some(&0) {
                bytes.pop();
            }
            let text = String::from_utf8(bytes)
                .map_err(|_| anyhow::anyhow!("invalid long archive path"))?;
            let path = PathBuf::from(text);
            checked_path(&path)?;
            long_name = Some(path);
            continue;
        }
        let path = match long_name.take() {
            Some(path) => path,
            None => entry.path().context("invalid transfer path")?.into_owned(),
        };
        checked_path(&path)?;
        ensure!(seen.insert(path.clone()), "duplicate archive path");
        let kind = entry.header().entry_type();
        ensure!(
            kind.is_file() || kind.is_dir(),
            "archive links and special files are forbidden"
        );
        ensure!(
            entry.header().link_name()?.is_none(),
            "archive link targets are forbidden"
        );
        let size = entry.size();
        if path == Path::new("manifest.json") {
            ensure!(
                manifest.is_none() && kind.is_file() && size <= MAX_MANIFEST,
                "invalid transfer manifest entry"
            );
            let mut data = Zeroizing::new(Vec::new());
            entry.read_to_end(&mut data)?;
            let parsed: Manifest = serde_json::from_slice(&data)
                .map_err(|_| anyhow::anyhow!("invalid transfer manifest"))?;
            validate_manifest(&parsed)?;
            manifest = Some(parsed);
            continue;
        }
        ensure!(manifest.is_some(), "manifest must precede browser files");
        ensure!(
            path.starts_with("chrome"),
            "archive contains data outside the selected Chrome directory"
        );
        let relative = path.strip_prefix("chrome")?;
        ensure!(
            !transient(relative),
            "archive contains forbidden Chrome runtime state"
        );
        budget.consume(size)?;
        let destination = stage.join(&path);
        store::check_dir(destination.parent().context("archive path has no parent")?)?;
        if kind.is_dir() {
            ensure!(size == 0, "directory entry contains data");
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&destination)
                .context("duplicate, aliased, or invalid archive directory")?;
        } else {
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&destination)
                .context("duplicate, aliased, or invalid archive file")?;
            ensure!(
                io::copy(&mut entry, &mut output)? == size,
                "incomplete archive file"
            );
        }
    }
    ensure!(long_name.is_none(), "incomplete long archive path");
    // tar stops before age's final authentication tag. Always consume to real EOF.
    let mut remaining = archive.into_inner();
    let mut trailing = 0usize;
    let mut buffer = [0; 4096];
    loop {
        let n = remaining
            .read(&mut buffer)
            .context("archive authentication failed or file is truncated")?;
        if n == 0 {
            break;
        }
        trailing += n;
        ensure!(
            trailing <= 1024 * 1024 && buffer[..n].iter().all(|b| *b == 0),
            "unexpected trailing archive data"
        );
    }
    let manifest = manifest.context("transfer manifest is missing")?;
    store::check_dir(&stage.join("chrome"))?;
    for snapshot in &manifest.cookie_stores {
        store::check_dir(&stage.join("chrome").join(&snapshot.directory))?;
    }
    Ok(manifest)
}

use std::os::unix::fs::DirBuilderExt;

fn restore_cookies(
    store: &Store,
    profile: &Profile,
    manifest: &Manifest,
    chrome: Option<&Path>,
) -> Result<usize> {
    let mut restored_count = 0;
    for snapshot in &manifest.cookie_stores {
        let dir = store.chrome_dir(profile).join(&snapshot.directory);
        // Remove only the imported copies of machine-encrypted cookie databases.
        // Chrome will recreate them using the destination's own encryption.
        for database in [dir.join("Cookies"), dir.join("Network/Cookies")] {
            for suffix in ["", "-journal", "-wal", "-shm"] {
                let mut path = database.as_os_str().to_os_string();
                path.push(suffix);
                match fs::remove_file(PathBuf::from(path)) {
                    Ok(()) => (),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                    Err(e) => return Err(e).context("cannot reset imported cookie database"),
                }
            }
        }
        browser::start_locked(store, profile, chrome, true, Some(&snapshot.directory))?;
        let restored = (|| -> Result<usize> {
            let (_, mut cdp) = browser::connect(store, profile)?;
            for batch in snapshot.cookies.chunks(100) {
                cdp.call("Storage.setCookies", json!({"cookies":batch}), None)?;
            }
            let actual = cdp.call("Storage.getCookies", json!({}), None)?;
            let actual = actual["cookies"]
                .as_array()
                .context("Chrome did not return restored cookie state")?;
            Ok(snapshot
                .cookies
                .iter()
                .filter(|wanted| {
                    actual.iter().any(|got| {
                        ["name", "value", "domain", "path", "partitionKey"]
                            .iter()
                            .all(|key| wanted[*key] == got[*key])
                    })
                })
                .count())
        })();
        let stopped = browser::stop_locked(store, profile);
        stopped.context(
            "Chrome did not stop after cookie restoration; no destination profile was installed",
        )?;
        restored_count += restored.context(
            "Chrome could not restore cookie state; no destination profile was installed",
        )?;
    }
    Ok(restored_count)
}

pub fn import(
    store: &Store,
    input: &Path,
    name: Option<&str>,
    port: Option<u16>,
    password: SecretString,
    max_bytes: u64,
    chrome: Option<&Path>,
) -> Result<Value> {
    ensure!(max_bytes > 0, "maximum transfer size must be positive");
    store.check()?;
    if let Some(name) = name {
        validate_name(name)?;
    }
    let file = secure_file(input)?;
    let stage = tempfile::Builder::new()
        .prefix(".abm-import-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(&store.root)?;
    let manifest = read_archive(BufReader::new(file), password, stage.path(), max_bytes)?;
    let name = name.unwrap_or(&manifest.profile);
    validate_name(name)?;
    ensure!(
        !store.profiles()?.iter().any(|p| p.name() == name)
            && store.root.join(name).symlink_metadata().is_err(),
        "destination profile already exists; import requires a new name"
    );
    let temporary = Store::new(Some(stage.path().join("managed")))?;
    temporary.init()?;
    let temporary_profile = temporary.create_profile("transfer-staging", None)?;
    fs::remove_dir(temporary.chrome_dir(&temporary_profile))?;
    fs::rename(
        stage.path().join("chrome"),
        temporary.chrome_dir(&temporary_profile),
    )?;
    let restored_count = {
        let _lock = temporary.lock(&temporary_profile, "lifecycle.lock", true)?;
        let result = restore_cookies(&temporary, &temporary_profile, &manifest, chrome);
        if browser::ensure_stopped_for_archive(&temporary, &temporary_profile).is_err() {
            let retained = stage.keep();
            return Err(anyhow::anyhow!(
                "temporary Chrome did not stop; private recovery data retained at {}; no destination profile was installed",
                retained.display()
            ));
        }
        result?
    };
    let installed = store.install_profile(name, port, &temporary.chrome_dir(&temporary_profile))?;
    let count: usize = manifest.cookie_stores.iter().map(|s| s.cookies.len()).sum();
    Ok(
        json!({"profile":installed.name(),"imported":true,"endpoint":installed.endpoint(),"portable_cookies":count,"cookies_restored":restored_count,"cookies_not_restored":count.saturating_sub(restored_count),"cookie_profiles":manifest.cookie_stores.len(),"skipped_opaque_cookies":manifest.skipped_opaque_cookies,"profile_state":"stopped","note":"Browser files and portable cookies restored. Device-bound logins and OS-protected passwords may require signing in again. Regenerate MCP config on this computer."}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    fn private_tempdir() -> Result<tempfile::TempDir> {
        Ok(tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()?)
    }
    fn secret() -> SecretString {
        SecretString::from("synthetic-test-passphrase-only".to_owned())
    }
    fn manifest() -> Manifest {
        Manifest {
            format: "agent-browser-manager".into(),
            version: 1,
            platform: std::env::consts::OS.into(),
            profile: "transfer-test".into(),
            cookie_stores: vec![CookieStore {
                directory: "Default".into(),
                cookies: vec![
                    json!({"name":"auth","value":"synthetic_cookie_do_not_print","domain":"example.test","path":"/","httpOnly":true,"secure":true}),
                ],
            }],
            skipped_opaque_cookies: 0,
        }
    }
    fn source() -> Result<tempfile::TempDir> {
        let temp = private_tempdir()?;
        store::make_private_dir(&temp.path().join("Default"))?;
        fs::write(
            temp.path().join("Default/Preferences"),
            b"{\"profile\":{\"name\":\"transfer-test\"}}",
        )?;
        fs::write(
            temp.path().join("Default/site-storage"),
            b"synthetic_local_storage_secret",
        )?;
        let deep = temp.path().join("Default").join("a".repeat(120));
        store::make_private_dir(&deep)?;
        fs::write(deep.join("value"), "preserved")?;
        Ok(temp)
    }
    #[test]
    fn encryption_round_trip_rejects_wrong_password_tampering_and_truncation() -> Result<()> {
        let src = source()?;
        let mut encrypted = Vec::new();
        write_archive(
            &mut encrypted,
            secret(),
            src.path(),
            &manifest(),
            1024 * 1024,
        )?;
        assert!(encrypted.starts_with(b"age-encryption.org/v1"));
        for text in [
            "synthetic_cookie_do_not_print",
            "synthetic_local_storage_secret",
            "transfer-test",
        ] {
            assert!(!encrypted.windows(text.len()).any(|w| w == text.as_bytes()));
        }
        let stage = private_tempdir()?;
        let result = read_archive(encrypted.as_slice(), secret(), stage.path(), 1024 * 1024)?;
        assert_eq!(result.profile, "transfer-test");
        assert_eq!(
            fs::read(stage.path().join("chrome/Default/site-storage"))?,
            b"synthetic_local_storage_secret"
        );
        assert_eq!(
            fs::read_to_string(
                stage
                    .path()
                    .join("chrome/Default")
                    .join("a".repeat(120))
                    .join("value")
            )?,
            "preserved"
        );
        assert_eq!(
            fs::metadata(stage.path().join("chrome/Default/site-storage"))?.mode() & 0o777,
            0o600
        );
        let wrong = private_tempdir()?;
        assert!(
            read_archive(
                encrypted.as_slice(),
                SecretString::from("different-test-passphrase".to_owned()),
                wrong.path(),
                1024 * 1024
            )
            .is_err()
        );
        assert!(fs::read_dir(wrong.path())?.next().is_none());
        let mut corrupt = encrypted.clone();
        *corrupt.last_mut().unwrap() ^= 1;
        for data in [
            &corrupt[..],
            &encrypted[..encrypted.len() - 1],
            &encrypted[..encrypted.len() / 2],
        ] {
            let stage = private_tempdir()?;
            assert!(read_archive(data, secret(), stage.path(), 1024 * 1024).is_err());
        }
        Ok(())
    }
    fn malicious(path: &str, kind: tar::EntryType, size: u64) -> Result<Vec<u8>> {
        let mut tar = tar::Builder::new(Vec::new());
        append_bytes(&mut tar, "manifest.json", &serde_json::to_vec(&manifest())?)?;
        for directory in ["chrome", "chrome/Default"] {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Directory);
            header.set_mode(0o700);
            header.set_size(0);
            header.set_cksum();
            tar.append_data(&mut header, directory, io::empty())?;
        }
        let mut header = tar::Header::new_gnu();
        header.set_size(size);
        header.set_mode(0o600);
        header.set_entry_type(kind);
        if kind.is_symlink() || kind.is_hard_link() {
            header.set_link_name("../../outside")?;
        }
        header.as_mut_bytes()[..100].fill(0);
        header.as_mut_bytes()[..path.len()].copy_from_slice(path.as_bytes());
        header.set_cksum();
        tar.append(&header, io::repeat(b'x').take(size))?;
        Ok(tar.into_inner()?)
    }
    #[test]
    fn extraction_rejects_traversal_links_special_files_and_size_bombs() -> Result<()> {
        for (path, kind, size) in [
            ("chrome/../outside", tar::EntryType::Regular, 1),
            ("/absolute", tar::EntryType::Regular, 1),
            ("other-config", tar::EntryType::Regular, 1),
            ("chrome/Default/link", tar::EntryType::Symlink, 0),
            ("chrome/Default/link", tar::EntryType::Link, 0),
            ("chrome/Default/device", tar::EntryType::Char, 0),
            ("chrome/Default/huge", tar::EntryType::Regular, 2048),
            ("chrome", tar::EntryType::Directory, 0),
            ("chrome/SingletonLock", tar::EntryType::Regular, 0),
            ("longname", tar::EntryType::GNULongName, 5000),
        ] {
            let data = malicious(path, kind, size)?;
            let temp = private_tempdir()?;
            assert!(
                extract_archive(data.as_slice(), temp.path(), 1024).is_err(),
                "accepted unsafe test entry"
            );
            assert!(!temp.path().join("outside").exists());
        }
        Ok(())
    }
    #[test]
    fn export_excludes_runtime_links_and_rejects_other_symlinks() -> Result<()> {
        let src = source()?;
        symlink("does-not-exist", src.path().join("SingletonLock"))?;
        let mut encrypted = Vec::new();
        write_archive(
            &mut encrypted,
            secret(),
            src.path(),
            &manifest(),
            1024 * 1024,
        )?;
        let dst = private_tempdir()?;
        read_archive(encrypted.as_slice(), secret(), dst.path(), 1024 * 1024)?;
        assert!(fs::symlink_metadata(dst.path().join("chrome/SingletonLock")).is_err());
        symlink("/etc/passwd", src.path().join("Default/unsafe-link"))?;
        assert!(write_archive(Vec::new(), secret(), src.path(), &manifest(), 1024 * 1024).is_err());
        Ok(())
    }
    #[test]
    fn rejects_bad_manifest_and_unsafe_passphrase_files() -> Result<()> {
        let mut m = manifest();
        m.version = 999;
        assert!(validate_manifest(&m).is_err());
        m.version = 1;
        m.cookie_stores[0].directory = "../escape".into();
        assert!(validate_manifest(&m).is_err());
        let temp = private_tempdir()?;
        let path = temp.path().join("key");
        fs::write(&path, "synthetic-test-passphrase-only\n")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644))?;
        assert!(passphrase(Some(&path), false).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        assert!(passphrase(Some(&path), false).is_ok());
        let link = temp.path().join("link");
        symlink(&path, &link)?;
        assert!(passphrase(Some(&link), false).is_err());
        Ok(())
    }
    #[test]
    fn cookie_conversion_preserves_session_flags_and_omits_opaque_partitions() -> Result<()> {
        let cookie = json!({"name":"sid","value":"synthetic","domain":"example.test","path":"/","session":true,"expires":-1,"httpOnly":true,"secure":true,"sameSite":"Strict","size":99,"priority":"High"});
        let converted = portable_cookie(&cookie)?.unwrap();
        assert!(converted.get("expires").is_none());
        assert!(converted.get("size").is_none());
        assert_eq!(converted["httpOnly"], true);
        let mut opaque = cookie;
        opaque["partitionKeyOpaque"] = json!(true);
        assert!(portable_cookie(&opaque)?.is_none());
        Ok(())
    }

    #[test]
    fn git_exports_require_an_ignored_directory_and_untracked_archive() -> Result<()> {
        let temp = private_tempdir()?;
        let root = temp.path();
        let git = |args: &[&str]| -> Result<()> {
            ensure!(
                Command::new("git")
                    .arg("-C")
                    .arg(root)
                    .args(args)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()?
                    .success(),
                "test Git command failed"
            );
            Ok(())
        };
        check_git_output(root, &root.join("outside.abm.age"))?;
        git(&["init", "--quiet"])?;
        fs::write(root.join(".gitignore"), "/exports/\n*.abm.age\n")?;
        let exports = root.join("exports");
        store::make_private_dir(&exports)?;
        check_git_output(&exports, &exports.join("profile.abm.age"))?;
        // A filename-only ignore rule leaves temporary archive files exposed.
        assert!(check_git_output(root, &root.join("profile.abm.age")).is_err());
        let tracked = exports.join("tracked.abm.age");
        fs::write(&tracked, "synthetic test data")?;
        git(&["add", "--force", "--", "exports/tracked.abm.age"])?;
        fs::remove_file(&tracked)?;
        assert!(check_git_output(&exports, &tracked).is_err());
        fs::write(root.join(".gitignore"), "*.abm.age\n")?;
        assert!(check_git_output(&exports, &exports.join("profile.abm.age")).is_err());
        Ok(())
    }

    #[test]
    #[ignore = "real Chrome transfer across internal profiles; requires macOS and free ports"]
    fn multiple_internal_profiles_keep_separate_cookie_jars() -> Result<()> {
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let Ok(store) = Store::new(Some(self.0.clone())) else {
                    return;
                };
                if let Ok(profiles) = store.profiles() {
                    for profile in profiles {
                        let _ = browser::stop(&store, &profile);
                    }
                }
            }
        }
        let temp = private_tempdir()?;
        let store = Store::new(Some(temp.path().join("runtime")))?;
        store.init()?;
        let _cleanup = Cleanup(store.root.clone());
        let profile = store.create_profile("multi-source", None)?;
        for (directory, value) in [("Default", "first"), ("Profile 1", "second")] {
            browser::start_locked(&store, &profile, None, true, Some(directory))?;
            let (_, mut cdp) = browser::connect(&store, &profile)?;
            cdp.call(
                "Storage.setCookies",
                json!({"cookies":[{
                    "name":"synthetic-auth", "value":value, "domain":"example.test",
                    "path":"/", "httpOnly":true, "secure":true
                }]}),
                None,
            )?;
            browser::stop_locked(&store, &profile)?;
        }
        browser::start_locked(&store, &profile, None, true, Some("Default"))?;
        let archive = temp.path().join("multiple.abm.age");
        let exported = export(
            &store,
            profile.name(),
            &archive,
            secret(),
            DEFAULT_MAX_BYTES,
            None,
            true,
        )?;
        ensure!(
            exported["cookie_profiles"] == 2,
            "missing internal cookie profile"
        );
        let imported = import(
            &store,
            &archive,
            Some("multi-copy"),
            None,
            secret(),
            DEFAULT_MAX_BYTES,
            None,
        )?;
        ensure!(
            imported["cookies_restored"] == 2,
            "missing restored test cookies"
        );
        let copy = store.resolve_profile("multi-copy")?;
        for (directory, value) in [("Default", "first"), ("Profile 1", "second")] {
            browser::start_locked(&store, &copy, None, true, Some(directory))?;
            let (_, mut cdp) = browser::connect(&store, &copy)?;
            let cookies = cdp.call("Storage.getCookies", json!({}), None)?;
            let values = cookies["cookies"]
                .as_array()
                .context("missing test cookie jar")?;
            ensure!(
                values
                    .iter()
                    .any(|cookie| cookie["name"] == "synthetic-auth" && cookie["value"] == value),
                "internal test cookie jar changed"
            );
            browser::stop_locked(&store, &copy)?;
        }
        Ok(())
    }
}
