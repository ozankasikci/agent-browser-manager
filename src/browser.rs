use crate::{
    Profile,
    cdp::{self, Cdp},
    store::{self, Store},
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    net::{Ipv4Addr, SocketAddr, TcpListener},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub schema: u32,
    pub pid: u32,
    pub fingerprint: String,
    pub profile_dir: PathBuf,
    pub chrome_path: PathBuf,
    pub port: u16,
    pub browser_path: String,
    pub run_id: String,
}

#[derive(Debug, PartialEq)]
pub struct Listener {
    pub pid: u32,
    pub address: String,
}

pub fn listeners(port: u16) -> Result<Vec<Listener>> {
    let result = Command::new("/usr/sbin/lsof")
        .args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-Fpn"])
        .output()
        .context("lsof is required to verify listener ownership")?;
    if result.status.code() == Some(1) && result.stdout.is_empty() && result.stderr.is_empty() {
        return Ok(Vec::new());
    }
    ensure!(
        result.status.success(),
        "could not inspect listening sockets; refusing browser access"
    );
    parse_listeners(std::str::from_utf8(&result.stdout)?)
}

fn parse_listeners(text: &str) -> Result<Vec<Listener>> {
    let mut pid = None;
    let mut result = Vec::new();
    for line in text.lines() {
        if let Some(s) = line.strip_prefix('p') {
            pid = Some(s.parse::<u32>().context("invalid listener PID")?);
        }
        if let Some(s) = line.strip_prefix('n') {
            result.push(Listener {
                pid: pid.context("listener has no PID")?,
                address: s.to_owned(),
            });
        }
    }
    ensure!(
        !result.is_empty(),
        "lsof returned no verifiable listener records"
    );
    Ok(result)
}

pub fn verify_listeners(found: &[Listener], pid: u32, port: u16) -> Result<()> {
    ensure!(!found.is_empty(), "managed Chrome has no CDP listener");
    for listener in found {
        ensure!(listener.pid == pid, "CDP port is owned by another process");
        ensure!(
            listener.address == format!("127.0.0.1:{port}")
                || listener.address == format!("[::1]:{port}"),
            "CDP is not bound exclusively to loopback; refusing access"
        );
    }
    Ok(())
}

fn process_field(pid: u32, field: &str) -> Result<Option<String>> {
    ensure!(pid > 1 && pid <= i32::MAX as u32, "invalid managed PID");
    let result = Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-ww", "-o", field])
        .output()?;
    if result.status.code() == Some(1) {
        return Ok(None);
    }
    ensure!(result.status.success(), "could not verify managed process");
    let text = String::from_utf8(result.stdout)?.trim().to_owned();
    Ok((!text.is_empty()).then_some(text))
}

fn fingerprint(pid: u32) -> Result<Option<String>> {
    process_field(pid, "lstart=")
}

fn alive(record: &Record) -> Result<bool> {
    Ok(fingerprint(record.pid)?.is_some_and(|value| value == record.fingerprint))
}

fn verify_process(record: &Record, store: &Store, profile: Profile) -> Result<()> {
    ensure!(
        record.schema == 1 && record.port == profile.port(),
        "managed state does not match profile"
    );
    ensure!(
        record.profile_dir == fs::canonicalize(store.chrome_dir(profile))?,
        "managed profile path changed"
    );
    ensure!(
        alive(record)?,
        "managed browser process is no longer running"
    );
    let command = process_field(record.pid, "command=")?.context("managed browser has exited")?;
    ensure!(
        command.starts_with(
            record
                .chrome_path
                .to_str()
                .context("Chrome path is not UTF-8")?
        ),
        "managed executable changed"
    );
    ensure!(
        command.contains(&format!("--user-data-dir={}", record.profile_dir.display()))
            && command.contains(&format!("--remote-debugging-port={}", profile.port())),
        "managed process arguments changed"
    );
    Ok(())
}

pub fn load(store: &Store, profile: Profile) -> Result<Option<Record>> {
    store::read_json(&store.path(profile, "browser.json"))
}

pub fn connect(store: &Store, profile: Profile) -> Result<(Record, Cdp)> {
    let record = load(store, profile)?.context("profile is stopped; run abm start first")?;
    verify_process(&record, store, profile)?;
    verify_listeners(&listeners(profile.port())?, record.pid, profile.port())?;
    let path = cdp::browser_path(profile.port())?;
    ensure!(
        !record.browser_path.is_empty() && path == record.browser_path,
        "CDP browser identity changed; refusing attachment"
    );
    let mut cdp = Cdp::connect(profile.port(), &path)?;
    let info = cdp.call("SystemInfo.getProcessInfo", json!({}), None)?;
    ensure!(
        info["processInfo"].as_array().is_some_and(|items| items
            .iter()
            .any(|v| v["type"] == "browser" && v["id"].as_f64() == Some(record.pid as f64))),
        "CDP browser PID does not match managed process"
    );
    Ok((record, cdp))
}

fn chrome_path(explicit: Option<&Path>) -> Result<PathBuf> {
    let path = explicit.unwrap_or_else(|| {
        Path::new("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")
    });
    ensure!(
        path.is_absolute(),
        "Chrome executable must be an absolute path"
    );
    let path = fs::canonicalize(path)
        .context("Chrome executable not found; use --chrome with an absolute path")?;
    ensure!(path.is_file(), "Chrome executable must be a file");
    Ok(path)
}

pub fn start(
    store: &Store,
    profile: Profile,
    chrome: Option<&Path>,
    headless: bool,
) -> Result<Value> {
    let _lock = store.lock(profile, "lifecycle.lock", true)?;
    if let Some(record) = load(store, profile)? {
        if alive(&record)? {
            connect(store, profile)?;
            return Ok(
                json!({"profile":profile.name(),"state":"running","already_running":true,"endpoint":profile.endpoint()}),
            );
        }
    }
    ensure!(
        listeners(profile.port())?.is_empty(),
        "fixed CDP port is already occupied; refusing to attach or terminate its owner"
    );
    // Also check that the IPv4 port can be bound. The guard is released immediately before spawn.
    let guard = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, profile.port())))
        .context("fixed CDP port is unavailable")?;
    let chrome_path = chrome_path(chrome)?;
    let profile_dir = fs::canonicalize(store.chrome_dir(profile))?;
    ensure!(
        profile_dir
            .join("SingletonLock")
            .symlink_metadata()
            .is_err(),
        "Chrome profile lock exists; close that profile normally before starting it"
    );
    let mut command = Command::new(&chrome_path);
    command.args([
        format!("--user-data-dir={}", profile_dir.display()),
        "--remote-debugging-address=127.0.0.1".to_owned(),
        format!("--remote-debugging-port={}", profile.port()),
        "--no-first-run".to_owned(),
        "--no-default-browser-check".to_owned(),
        "--disable-extensions".to_owned(),
        "--disable-sync".to_owned(),
        "--disable-background-mode".to_owned(),
    ]);
    if headless {
        command.arg("--headless=new");
    }
    command
        .arg("about:blank")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    drop(guard);
    let mut child = command.spawn().context("cannot launch Chrome")?;
    let result = (|| -> Result<Value> {
        let mut record = Record {
            schema: 1,
            pid: child.id(),
            fingerprint: fingerprint(child.id())?
                .context("Chrome exited before its process could be recorded")?,
            profile_dir,
            chrome_path,
            port: profile.port(),
            browser_path: String::new(),
            run_id: Uuid::new_v4().to_string(),
        };
        store::write_json(&store.path(profile, "browser.json"), &record)?;
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            ensure!(
                child.try_wait()?.is_none(),
                "Chrome exited before CDP was ready"
            );
            let found = listeners(profile.port())?;
            if !found.is_empty() {
                verify_listeners(&found, record.pid, profile.port())?;
                if let Ok(path) = cdp::browser_path(profile.port()) {
                    record.browser_path = path;
                    store::write_json(&store.path(profile, "browser.json"), &record)?;
                    connect(store, profile)?;
                    return Ok(
                        json!({"profile":profile.name(),"state":"running","already_running":false,"endpoint":profile.endpoint()}),
                    );
                }
            }
            ensure!(
                Instant::now() < deadline,
                "Chrome did not expose CDP within 15 seconds"
            );
            thread::sleep(Duration::from_millis(100));
        }
    })();
    if result.is_err() {
        // This is the child we just spawned, not a PID taken from an untrusted port.
        let _ = child.kill();
        let _ = child.wait();
        let _ = fs::remove_file(store.path(profile, "browser.json"));
    }
    result
}

pub fn stop(store: &Store, profile: Profile) -> Result<Value> {
    let _lock = store.lock(profile, "lifecycle.lock", true)?;
    let found = listeners(profile.port())?;
    let Some(record) = load(store, profile)? else {
        ensure!(
            found.is_empty(),
            "CDP port is occupied by an unmanaged process; refusing to stop it"
        );
        return Ok(json!({"profile":profile.name(),"state":"stopped","already_stopped":true}));
    };
    if !alive(&record)? {
        ensure!(
            found.is_empty(),
            "stale browser record and occupied CDP port; refusing to stop its owner"
        );
        fs::remove_file(store.path(profile, "browser.json"))?;
        return Ok(json!({"profile":profile.name(),"state":"stopped","removed_stale_record":true}));
    }
    verify_process(&record, store, profile)?;
    if !found.is_empty() {
        verify_listeners(&found, record.pid, profile.port())?;
        // A live but changed browser identity must never receive Browser.close.
        let (_, mut cdp) = connect(store, profile)?;
        let _ = cdp.call("Browser.close", json!({}), None);
    } else {
        // Recover a managed process interrupted before CDP startup. Never signal an arbitrary PID.
        signal_managed(&record, store, profile)?;
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if !alive(&record)? {
            ensure!(
                listeners(profile.port())?.is_empty(),
                "another process took the CDP port while stopping"
            );
            fs::remove_file(store.path(profile, "browser.json"))?;
            return Ok(json!({"profile":profile.name(),"state":"stopped"}));
        }
        if Instant::now() >= deadline {
            bail!("Chrome has not exited; browser state was retained for safe recovery");
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn signal_managed(record: &Record, store: &Store, profile: Profile) -> Result<()> {
    verify_process(record, store, profile)?;
    // SAFETY: PID > 1, positive i32, process start time and command have just been verified.
    ensure!(
        unsafe { libc::kill(record.pid as i32, libc::SIGTERM) } == 0,
        "could not terminate managed Chrome"
    );
    Ok(())
}

pub fn status(store: &Store, profile: Profile) -> Result<Value> {
    let _lock = store.lock(profile, "lifecycle.lock", false)?;
    let found = listeners(profile.port())?;
    let record = load(store, profile)?;
    let state = match record {
        Some(record) if alive(&record)? => {
            connect(store, profile)?;
            "running"
        }
        Some(_) if found.is_empty() => "stale",
        None if found.is_empty() => "stopped",
        _ => bail!(
            "profile {} has an occupied CDP port without a matching managed process",
            profile.name()
        ),
    };
    Ok(json!({"profile":profile.name(),"state":state,"endpoint":profile.endpoint()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn listener_audit_rejects_wildcard_wrong_pid_and_unexpected_address() {
        for address in [
            "*:9222",
            "0.0.0.0:9222",
            "[::]:9222",
            "192.168.1.2:9222",
            "127.0.0.1:9223",
        ] {
            assert!(
                verify_listeners(
                    &[Listener {
                        pid: 42,
                        address: address.into()
                    }],
                    42,
                    9222
                )
                .is_err()
            );
        }
        assert!(
            verify_listeners(
                &[Listener {
                    pid: 43,
                    address: "127.0.0.1:9222".into()
                }],
                42,
                9222
            )
            .is_err()
        );
        assert!(verify_listeners(&[], 42, 9222).is_err());
        assert!(
            verify_listeners(
                &[
                    Listener {
                        pid: 42,
                        address: "127.0.0.1:9222".into()
                    },
                    Listener {
                        pid: 42,
                        address: "[::1]:9222".into()
                    }
                ],
                42,
                9222
            )
            .is_ok()
        );
    }
    #[test]
    fn parses_machine_readable_lsof() -> Result<()> {
        assert_eq!(
            parse_listeners("p42\nn127.0.0.1:9222\nn[::1]:9222\n")?.len(),
            2
        );
        assert!(parse_listeners("n*:9222\n").is_err());
        Ok(())
    }
}
