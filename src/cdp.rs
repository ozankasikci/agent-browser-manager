use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpStream},
    time::{Duration, Instant},
};
use tungstenite::{Message, WebSocket, protocol::WebSocketConfig};
use url::Url;

const TIMEOUT: Duration = Duration::from_secs(8);
const MAX_HTTP: usize = 1024 * 1024;

pub fn socket(port: u16) -> Result<TcpStream> {
    let stream = TcpStream::connect_timeout(
        &SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        Duration::from_secs(2),
    )
    .context("cannot connect to managed Chrome on localhost")?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    Ok(stream)
}

pub fn browser_path(port: u16) -> Result<String> {
    let mut stream = socket(port)?;
    write!(
        stream,
        "GET /json/version HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    )?;
    let value = discovery_response(&mut stream)?;
    let url = value["webSocketDebuggerUrl"]
        .as_str()
        .context("CDP browser endpoint missing")?;
    validate_ws_url(url, port)
}

fn discovery_response(stream: &mut impl Read) -> Result<Value> {
    // Chrome can keep discovery sockets alive even with Connection: close.
    // Read exactly Content-Length rather than waiting for EOF.
    let mut data = Vec::new();
    while !data.ends_with(b"\r\n\r\n") {
        ensure!(
            data.len() < 16 * 1024,
            "CDP discovery headers are too large"
        );
        let mut byte = [0];
        stream
            .read_exact(&mut byte)
            .context("incomplete CDP discovery headers")?;
        data.push(byte[0]);
    }
    let headers = std::str::from_utf8(&data)?;
    ensure!(
        headers
            .lines()
            .next()
            .is_some_and(|s| s.starts_with("HTTP/1.1 200 ") || s.starts_with("HTTP/1.0 200 ")),
        "CDP discovery refused"
    );
    ensure!(
        !headers.to_ascii_lowercase().contains("transfer-encoding:"),
        "unsupported CDP discovery transfer encoding"
    );
    let lengths: Vec<_> = headers
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then_some(value.trim())
        })
        .collect();
    ensure!(
        lengths.len() == 1,
        "CDP discovery requires one Content-Length header"
    );
    let length: usize = lengths[0].parse().context("invalid CDP Content-Length")?;
    ensure!(length <= MAX_HTTP, "CDP discovery response is too large");
    let mut body = vec![0; length];
    stream
        .read_exact(&mut body)
        .context("incomplete CDP discovery body")?;
    serde_json::from_slice(&body).context("invalid CDP discovery data")
}

pub fn validate_ws_url(raw: &str, port: u16) -> Result<String> {
    let u = Url::parse(raw).context("invalid CDP endpoint")?;
    ensure!(
        u.scheme() == "ws"
            && matches!(u.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
            && u.port() == Some(port),
        "CDP endpoint must use the expected loopback port"
    );
    ensure!(
        u.username().is_empty()
            && u.password().is_none()
            && u.query().is_none()
            && u.fragment().is_none(),
        "unexpected CDP endpoint components"
    );
    let id = u
        .path()
        .strip_prefix("/devtools/browser/")
        .context("not a browser CDP endpoint")?;
    ensure!(
        !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'),
        "invalid CDP browser identifier"
    );
    Ok(u.path().to_owned())
}

pub struct Cdp {
    ws: WebSocket<TcpStream>,
    next_id: u64,
}

impl Cdp {
    pub fn connect(port: u16, path: &str) -> Result<Self> {
        let raw = format!("ws://127.0.0.1:{port}{path}");
        validate_ws_url(&raw, port)?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(8 * 1024 * 1024))
            .max_frame_size(Some(8 * 1024 * 1024));
        let (ws, _) =
            tungstenite::client::client_with_config(raw.as_str(), socket(port)?, Some(config))
                .map_err(|_| anyhow::anyhow!("CDP websocket handshake failed"))?;
        Ok(Self { ws, next_id: 0 })
    }

    pub fn call(&mut self, method: &str, params: Value, session: Option<&str>) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let mut request = json!({"id":id,"method":method,"params":params});
        if let Some(session) = session {
            request["sessionId"] = json!(session);
        }
        self.ws
            .send(Message::Text(request.to_string().into()))
            .map_err(|_| anyhow::anyhow!("CDP write failed"))?;
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            ensure!(!remaining.is_zero(), "CDP operation timed out");
            self.ws.get_mut().set_read_timeout(Some(remaining))?;
            match self
                .ws
                .read()
                .map_err(|_| anyhow::anyhow!("CDP read failed or timed out"))?
            {
                Message::Text(text) => {
                    let value: Value = serde_json::from_str(&text).context("invalid CDP JSON")?;
                    if value["id"] == id {
                        if value.get("error").is_some() {
                            bail!("Chrome rejected {method} (code {})", value["error"]["code"]);
                        }
                        return Ok(value["result"].clone());
                    }
                }
                Message::Ping(data) => self.ws.send(Message::Pong(data))?,
                Message::Close(_) => bail!("Chrome closed the CDP connection"),
                _ => (),
            }
        }
    }

    pub fn attach(&mut self, target: &str) -> Result<String> {
        let info = self.call("Target.getTargetInfo", json!({"targetId":target}), None)?;
        ensure!(info["targetInfo"]["type"] == "page", "target is not a page");
        let v = self.call(
            "Target.attachToTarget",
            json!({"targetId":target,"flatten":true}),
            None,
        )?;
        Ok(v["sessionId"]
            .as_str()
            .context("missing CDP session")?
            .to_owned())
    }
}

pub fn validate_navigation(raw: &str) -> Result<()> {
    ensure!(raw.len() <= 4096, "URL is too long");
    if raw == "about:blank" {
        return Ok(());
    }
    let u = Url::parse(raw).context("URL must be absolute HTTP(S) or about:blank")?;
    ensure!(
        matches!(u.scheme(), "http" | "https") && u.host_str().is_some(),
        "only HTTP(S) or about:blank navigation is allowed"
    );
    ensure!(
        u.username().is_empty() && u.password().is_none(),
        "URL credentials are not allowed"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_respects_content_length_without_waiting_for_eof() -> Result<()> {
        let mut input = std::io::Cursor::new(
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}socket-remains-open",
        );
        assert_eq!(discovery_response(&mut input)?, json!({}));
        assert!(input.position() < input.get_ref().len() as u64);
        for bad in [
            "HTTP/1.1 302 Found\r\nContent-Length: 2\r\n\r\n{}",
            "HTTP/1.1 200 OK\r\nContent-Length: 999999999\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Length: 2\r\n\r\n{}",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n",
        ] {
            assert!(discovery_response(&mut std::io::Cursor::new(bad)).is_err());
        }
        Ok(())
    }
    #[test]
    fn rejects_remote_and_wrong_port_endpoints() {
        for raw in [
            "ws://evil.test:9222/devtools/browser/abcd",
            "ws://0.0.0.0:9222/devtools/browser/abcd",
            "ws://127.0.0.1:9223/devtools/browser/abcd",
            "ws://user@127.0.0.1:9222/devtools/browser/abcd",
            "ws://127.0.0.1:9222/devtools/page/abcd",
            "ws://127.0.0.1:9222/devtools/browser/abcd?q=1",
            "wss://127.0.0.1:9222/devtools/browser/abcd",
        ] {
            assert!(validate_ws_url(raw, 9222).is_err(), "{raw}");
        }
        assert!(validate_ws_url("ws://localhost:9222/devtools/browser/abcd-1234", 9222).is_ok());
    }
    #[test]
    fn navigation_is_restricted() {
        for raw in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,secret",
            "chrome://settings",
            "https://user:secret@example.com",
            "about:config",
        ] {
            assert!(validate_navigation(raw).is_err());
        }
        for raw in [
            "about:blank",
            "https://example.com",
            "http://127.0.0.1:8080",
        ] {
            assert!(validate_navigation(raw).is_ok());
        }
    }
}
