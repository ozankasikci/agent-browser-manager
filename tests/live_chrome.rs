//! Explicitly opt in: this test launches signed-out Chrome on the three fixed ports.
//! All state is in a temporary private directory. It never uses normal Chrome data.
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant},
};

const BIN: &str = env!("CARGO_BIN_EXE_abm");
const PROFILES: [&str; 3] = ["personal", "business", "development"];

fn invoke(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(BIN)
        .arg("--data-dir")
        .arg(root)
        .args(args)
        .output()
        .expect("run abm")
}
fn cli(root: &Path, args: &[&str]) -> Value {
    let result = invoke(root, args);
    assert!(
        result.status.success(),
        "abm {args:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    serde_json::from_slice(&result.stdout).expect("CLI JSON output")
}
struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        for p in PROFILES
            .into_iter()
            .chain(["studio-team", "transfer-source", "transfer-imported"])
        {
            let _ = invoke(&self.0, &["stop", p]);
        }
    }
}

struct Client {
    child: Child,
    input: Option<ChildStdin>,
    messages: Receiver<Value>,
    next_id: u64,
}
impl Client {
    fn new(root: &Path, profile: &str) -> Self {
        let mut child = Command::new(BIN)
            .arg("--data-dir")
            .arg(root)
            .args(["mcp", "--profile", profile, "--agent", "same-label"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let (tx, messages) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let line = line.unwrap();
                let value: Value =
                    serde_json::from_str(&line).expect("stdout must contain only JSON-RPC");
                if tx.send(value).is_err() {
                    break;
                }
            }
        });
        let mut c = Self {
            child,
            input: Some(input),
            messages,
            next_id: 0,
        };
        let init = c.request("initialize",json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"rust-test","version":"1"}}));
        assert_eq!(
            init["result"]["serverInfo"]["name"],
            "agent-browser-manager"
        );
        c.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        c
    }
    fn send(&mut self, value: Value) {
        writeln!(self.input.as_mut().unwrap(), "{value}").unwrap();
        self.input.as_mut().unwrap().flush().unwrap();
    }
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        self.send(json!({"jsonrpc":"2.0","id":self.next_id,"method":method,"params":params}));
        let response = self
            .messages
            .recv_timeout(Duration::from_secs(40))
            .expect("bounded MCP response time");
        assert_eq!(response["id"], self.next_id);
        response
    }
    fn raw_tool(&mut self, name: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name":name,"arguments":arguments}))["result"].clone()
    }
    fn tool(&mut self, name: &str, arguments: Value) -> Value {
        let result = self.raw_tool(name, arguments);
        assert_eq!(result["isError"], false, "{name}: {result}");
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
    }
    fn read_until(&mut self, target: &str, expected: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let result = self.tool("read_page", json!({"target_id":target}));
            if result.to_string().contains(expected) {
                return result;
            }
            assert!(Instant::now() < deadline, "page did not contain {expected}");
            thread::sleep(Duration::from_millis(100));
        }
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.input.take();
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Fixture {
    url: String,
    shutdown: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Fixture {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let stop = shutdown.clone();
        let thread = thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((s, _)) => respond(s),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10))
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            url,
            shutdown,
            thread: Some(thread),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            t.join().unwrap();
        }
    }
}
fn respond(mut stream: TcpStream) {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut reader = BufReader::new(&stream);
    let mut request = String::new();
    if reader.read_line(&mut request).is_err() {
        return;
    }
    let route = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
    let mut cookie = "none".to_owned();
    let mut session_cookie = false;
    for _ in 0..100 {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
            break;
        }
        if line.to_ascii_lowercase().starts_with("cookie:") {
            for part in line[7..].trim().split(';') {
                if part.trim() == "abm_session=synthetic-session" {
                    session_cookie = true;
                }
                if let Some(value) = part.trim().strip_prefix("abm_fixture=") {
                    cookie = value.to_owned();
                }
            }
        }
    }
    let value = route
        .strip_prefix("/set-cookie?who=")
        .filter(|s| PROFILES.contains(s));
    let set_cookie = value.map_or_else(String::new, |v| {
        format!("Set-Cookie: abm_fixture={v}; Path=/; Max-Age=3600; HttpOnly; SameSite=Strict\r\nSet-Cookie: abm_session=synthetic-session; Path=/; HttpOnly; SameSite=Strict\r\n")
    });
    let submitted = route.contains("note=helloRust");
    let stored = value.unwrap_or("none");
    // Runs only inside Chrome's localhost fixture. No separate JS runtime is used.
    let storage_script = if route.starts_with("/storage-") {
        r#"<script>
const request = indexedDB.open('abm_transfer_test', 1);
request.onupgradeneeded = () => request.result.createObjectStore('state');
request.onsuccess = () => {
  const db = request.result;
  if (location.pathname === '/storage-set') {
    localStorage.setItem('abm-transfer', 'persisted-local');
    const tx = db.transaction('state', 'readwrite');
    tx.objectStore('state').put('persisted-indexed', 'auth');
    tx.oncomplete = () => { db.close(); document.body.append('storage saved'); };
  } else {
    const read = db.transaction('state').objectStore('state').get('auth');
    read.onsuccess = () => { document.body.append('local storage: ' + localStorage.getItem('abm-transfer') + '; indexed db: ' + read.result); db.close(); };
  }
};
</script>"#
    } else {
        ""
    };
    let body = format!(
        "<!doctype html><html><head><title>ABM local fixture</title></head><body><h1>ABM fixture ready</h1><p>cookie: {cookie}</p><p>session cookie: {session_cookie}</p><p>cookie stored for: {stored}</p><p>submitted: {submitted}</p><form method=get action=/><label>Note <input id=note name=note></label><button id=save type=submit>Save fixture</button></form>{storage_script}</body></html>"
    );
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n{set_cookie}\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

#[test]
#[ignore = "launches real Chrome; requires macOS and free ports 9222, 9223, 9224"]
fn three_profiles_mcp_ownership_and_persistence() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("runtime");
    cli(&root, &["init"]);
    // Fail before starting anything if any fixed endpoint is already in use.
    let status = cli(&root, &["status"]);
    assert!(
        status
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v["state"] == "stopped")
    );
    let _cleanup = Cleanup(root.clone());
    let fixture = Fixture::new();

    // An unrelated listener must never be adopted or stopped.
    let blocker = TcpListener::bind("127.0.0.1:9224").unwrap();
    assert!(
        !invoke(&root, &["start", "development", "--headless"])
            .status
            .success()
    );
    assert!(!invoke(&root, &["stop", "development"]).status.success());
    assert!(blocker.local_addr().is_ok());
    drop(blocker);
    assert!(
        !invoke(
            &root,
            &[
                "start",
                "development",
                "--headless",
                "--chrome",
                "/usr/bin/false"
            ]
        )
        .status
        .success()
    );
    assert_eq!(
        cli(&root, &["status", "development"])[0]["state"],
        "stopped"
    );

    // Two CLI processes racing to start the same profile must launch just one Chrome.
    let mut first = Command::new(BIN)
        .arg("--data-dir")
        .arg(&root)
        .args(["start", "personal", "--headless"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let second = cli(&root, &["start", "personal", "--headless"]);
    let first_result = first.wait().unwrap();
    assert!(first_result.success());
    let mut first_output = String::new();
    first
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut first_output)
        .unwrap();
    let first_json: Value = serde_json::from_str(&first_output).unwrap();
    assert_ne!(first_json["already_running"], second["already_running"]);
    for profile in ["business", "development"] {
        cli(&root, &["start", profile, "--headless"]);
    }
    let status = cli(&root, &["status"]);
    assert!(
        status
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v["state"] == "running")
    );

    for profile in PROFILES {
        let mut a = Client::new(&root, profile);
        let mut b = Client::new(&root, profile);
        let catalog = a.request("tools/list", json!({}));
        assert_eq!(catalog["result"]["tools"].as_array().unwrap().len(), 8);
        let open = a.tool("open_tab", json!({"url":format!("{}/",fixture.url)}));
        let target = open["target_id"].as_str().unwrap().to_owned();
        a.read_until(&target, "cookie: none");
        a.tool(
            "navigate",
            json!({"target_id":target,"url":format!("{}/set-cookie?who={profile}",fixture.url)}),
        );
        a.read_until(&target, &format!("cookie stored for: {profile}"));
        a.tool(
            "navigate",
            json!({"target_id":target,"url":format!("{}/",fixture.url)}),
        );
        a.read_until(&target, &format!("cookie: {profile}"));
        assert_eq!(
            a.tool("list_tabs", json!({}))["target_ids"],
            json!([target])
        );
        assert_eq!(b.tool("list_tabs", json!({}))["target_ids"], json!([]));
        for (name, args) in [
            ("navigate", json!({"target_id":target,"url":"about:blank"})),
            ("read_page", json!({"target_id":target})),
            ("click", json!({"target_id":target,"selector":"#save"})),
            (
                "type_text",
                json!({"target_id":target,"selector":"#note","text":"intruder"}),
            ),
            ("close_tab", json!({"target_id":target})),
        ] {
            assert_eq!(
                b.raw_tool(name, args)["isError"],
                true,
                "foreign ownership must reject {name}"
            );
        }
        assert_eq!(
            a.raw_tool(
                "navigate",
                json!({"target_id":target,"url":"file:///etc/passwd"})
            )["isError"],
            true
        );
        if profile == "development" {
            a.tool(
                "type_text",
                json!({"target_id":target,"selector":"#note","text":"helloRust"}),
            );
            a.tool("click", json!({"target_id":target,"selector":"#save"}));
            a.read_until(&target, "submitted: true");
        }
        a.tool("close_tab", json!({"target_id":target}));
        assert_eq!(a.tool("list_tabs", json!({}))["target_ids"], json!([]));
        // Losing a client never grants a new client its tabs, even with the same agent label.
        let orphan = a.tool("open_tab", json!({}))["target_id"]
            .as_str()
            .unwrap()
            .to_owned();
        drop(a);
        let mut c = Client::new(&root, profile);
        assert_eq!(
            c.raw_tool("close_tab", json!({"target_id":orphan}))["isError"],
            true
        );
    }
    for profile in PROFILES {
        cli(&root, &["stop", profile]);
    }
    for profile in PROFILES {
        cli(&root, &["start", profile, "--headless"]);
        let mut c = Client::new(&root, profile);
        assert_eq!(c.tool("list_tabs", json!({}))["target_ids"], json!([]));
        let target = c.tool("open_tab", json!({"url":format!("{}/",fixture.url)}))["target_id"]
            .as_str()
            .unwrap()
            .to_owned();
        c.read_until(&target, &format!("cookie: {profile}"));
        c.tool("close_tab", json!({"target_id":target}));
        cli(&root, &["stop", profile]);
    }
    assert!(
        cli(&root, &["status"])
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v["state"] == "stopped")
    );

    // A user-named profile has its own directory, stable endpoint, MCP scope, and Chrome name.
    let launcher = Command::new(BIN)
        .arg("--data-dir")
        .arg(&root)
        .args(["start", "studio-team", "--headless"])
        .process_group(0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let launcher_group = i32::try_from(launcher.id()).unwrap();
    let output = launcher.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let started: Value = serde_json::from_slice(&output.stdout).unwrap();
    // Simulate terminal/agent cleanup after the short-lived launcher exits.
    // SAFETY: this is the isolated process group created for this test launcher.
    unsafe {
        libc::kill(-launcher_group, libc::SIGTERM);
    }
    thread::sleep(Duration::from_millis(150));
    assert_eq!(
        cli(&root, &["status", "studio-team"])[0]["state"],
        "running"
    );
    assert!(
        !invoke(&root, &["profile", "remove", "studio-team"])
            .status
            .success()
    );
    assert_eq!(
        cli(&root, &["status", "studio-team"])[0]["state"],
        "running"
    );
    assert_eq!(started["profile"], "studio-team");
    let endpoint = started["endpoint"].clone();
    let mut a = Client::new(&root, "studio-team");
    let mut b = Client::new(&root, "studio-team");
    let target = a.tool("open_tab", json!({"url":format!("{}/", fixture.url)}))["target_id"]
        .as_str()
        .unwrap()
        .to_owned();
    a.read_until(&target, "cookie: none");
    assert_eq!(
        b.raw_tool("read_page", json!({"target_id":target}))["isError"],
        true
    );
    drop(a);
    drop(b);
    cli(&root, &["stop", "studio-team"]);
    let prefs: Value = serde_json::from_slice(
        &std::fs::read(root.join("studio-team/chrome/Default/Preferences")).unwrap(),
    )
    .unwrap();
    assert_eq!(prefs["profile"]["name"], "studio-team");
    let local: Value = serde_json::from_slice(
        &std::fs::read(root.join("studio-team/chrome/Local State")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        local["profile"]["info_cache"]["Default"]["name"],
        "studio-team"
    );
    assert_eq!(
        cli(&root, &["start", "studio-team", "--headless"])["endpoint"],
        endpoint
    );
    cli(&root, &["stop", "studio-team"]);
    let removed = cli(&root, &["profile", "remove", "studio-team"]);
    let archived = Path::new(removed["archive"].as_str().unwrap());
    assert!(archived.join("chrome/Default/Preferences").is_file());
    assert!(!root.join("studio-team").exists());
}

#[test]
#[ignore = "real Chrome encrypted transfer; requires macOS and free managed ports"]
fn encrypted_whole_profile_transfer_preserves_cookie_and_site_storage() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("runtime");
    cli(&root, &["init"]);
    let _cleanup = Cleanup(root.clone());
    let fixture = Fixture::new();
    cli(&root, &["start", "transfer-source", "--headless"]);
    let mut client = Client::new(&root, "transfer-source");
    let target = client.tool(
        "open_tab",
        json!({"url":format!("{}/set-cookie?who=personal",fixture.url)}),
    )["target_id"]
        .as_str()
        .unwrap()
        .to_owned();
    client.read_until(&target, "cookie stored for: personal");
    client.tool(
        "navigate",
        json!({"target_id":target,"url":format!("{}/storage-set",fixture.url)}),
    );
    client.read_until(&target, "storage saved");
    let key = temp.path().join("key");
    std::fs::write(&key, "synthetic-transfer-passphrase-only\n").unwrap();
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    let archive = temp.path().join("profile.abm.age");
    let export_args = [
        "profile",
        "export",
        "transfer-source",
        "--output",
        archive.to_str().unwrap(),
        "--passphrase-file",
        key.to_str().unwrap(),
    ];
    // A running source is never silently stopped or copied mid-write.
    assert!(!invoke(&root, &export_args).status.success());
    assert!(!archive.exists());
    std::fs::write(
        root.join("transfer-source/chrome/transfer-marker"),
        "whole-profile-file",
    )
    .unwrap();
    let mut live_export_args = export_args.to_vec();
    live_export_args.push("--stop");
    let exported = cli(&root, &live_export_args);
    drop(client);
    assert!(exported["portable_cookies"].as_u64().unwrap() >= 2);
    assert_eq!(exported["profile_state"], "stopped");
    assert_eq!(
        std::fs::metadata(&archive).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(!invoke(&root, &export_args).status.success());
    cli(&root, &["start", "transfer-source", "--headless"]);
    let mut source_again = Client::new(&root, "transfer-source");
    let source_target = source_again.tool(
        "open_tab",
        json!({"url":format!("{}/storage-read",fixture.url)}),
    )["target_id"]
        .as_str()
        .unwrap()
        .to_owned();
    source_again.read_until(&source_target, "session cookie: true");
    cli(&root, &["stop", "transfer-source"]);
    drop(source_again);
    // A wrong password or existing destination cannot publish an imported profile.
    let wrong = temp.path().join("wrong-key");
    std::fs::write(&wrong, "another-synthetic-passphrase\n").unwrap();
    std::fs::set_permissions(&wrong, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        !invoke(
            &root,
            &[
                "profile",
                "import",
                archive.to_str().unwrap(),
                "--name",
                "transfer-imported",
                "--passphrase-file",
                wrong.to_str().unwrap()
            ]
        )
        .status
        .success()
    );
    assert!(!root.join("transfer-imported").exists());
    assert!(
        !invoke(
            &root,
            &[
                "profile",
                "import",
                archive.to_str().unwrap(),
                "--name",
                "transfer-source",
                "--passphrase-file",
                key.to_str().unwrap()
            ]
        )
        .status
        .success()
    );
    let imported = cli(
        &root,
        &[
            "profile",
            "import",
            archive.to_str().unwrap(),
            "--name",
            "transfer-imported",
            "--passphrase-file",
            key.to_str().unwrap(),
        ],
    );
    assert!(imported["cookies_restored"].as_u64().unwrap() >= 2);
    assert_eq!(
        std::fs::read_to_string(root.join("transfer-imported/chrome/transfer-marker")).unwrap(),
        "whole-profile-file"
    );
    assert!(!root.join("transfer-imported/ownership.json").exists());
    assert!(!root.join("transfer-imported/browser.json").exists());
    cli(&root, &["start", "transfer-imported", "--headless"]);
    let mut imported_client = Client::new(&root, "transfer-imported");
    let target = imported_client.tool(
        "open_tab",
        json!({"url":format!("{}/storage-read",fixture.url)}),
    )["target_id"]
        .as_str()
        .unwrap()
        .to_owned();
    imported_client.read_until(&target, "cookie: personal");
    imported_client.read_until(&target, "session cookie: true");
    imported_client.read_until(
        &target,
        "local storage: persisted-local; indexed db: persisted-indexed",
    );
    cli(&root, &["stop", "transfer-imported"]);
    drop(imported_client);
    assert!(std::fs::read_dir(&root).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".abm-import-")
    }));
}
