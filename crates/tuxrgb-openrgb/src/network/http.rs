//! A tiny HTTP/1.0 client for the local network: WLED, Philips Hue,
//! Nanoleaf and Elgato all speak plain HTTP with small JSON bodies, which
//! does not justify a full HTTP stack. Everything is bounded by a deadline.

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// Largest body accepted (a Hue bridge's full state stays well below).
const MAX_BODY: usize = 512 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

impl Response {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn json(&self) -> Option<serde_json::Value> {
        serde_json::from_str(self.body.trim()).ok()
    }
}

/// `host` as typed by the user (`192.168.1.40`, `name.local`,
/// `192.168.1.40:8080`, `[fe80::1]:80`) with `default_port` when it names
/// none: the address to connect to and the name for the `Host` header.
pub fn target(host: &str, default_port: u16) -> Result<(SocketAddr, String), String> {
    let host = host.trim();
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok((SocketAddr::new(ip, default_port), host.to_owned()));
    }
    if let Ok(addr) = host.parse::<SocketAddr>() {
        return Ok((addr, addr.ip().to_string()));
    }
    let (name, port) = match host.rsplit_once(':') {
        Some((n, p)) if !n.contains(':') => (
            n,
            p.parse::<u16>()
                .map_err(|_| "adresse invalide".to_owned())?,
        ),
        _ => (host, default_port),
    };
    let addr = (name, port)
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .next()
        .ok_or_else(|| "adresse introuvable".to_owned())?;
    Ok((addr, name.to_owned()))
}

/// One request (`body`: JSON), the whole exchange within `timeout`.
pub fn request(
    host: &str,
    default_port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
    timeout: Duration,
) -> Result<Response, String> {
    if !super::valid_host(host) {
        return Err("adresse invalide".into());
    }
    let deadline = Instant::now() + timeout;
    let (addr, name) = target(host, default_port)?;
    let left = || {
        deadline
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(1))
    };
    let mut stream = TcpStream::connect_timeout(&addr, left()).map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(left()))
        .map_err(|e| e.to_string())?;
    let host_header = if addr.port() == 80 {
        name
    } else {
        format!("{name}:{}", addr.port())
    };
    let mut head = format!(
        "{method} {path} HTTP/1.0\r\nHost: {host_header}\r\nAccept: application/json\r\nConnection: close\r\n"
    );
    if let Some(b) = body {
        head.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            b.len()
        ));
    }
    head.push_str("\r\n");
    stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(body.unwrap_or_default().as_bytes()))
        .map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        stream
            .set_read_timeout(Some(left()))
            .map_err(|e| e.to_string())?;
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                raw.extend_from_slice(&buf[..n]);
                if raw.len() > MAX_BODY {
                    return Err("réponse trop longue".into());
                }
                if complete(&raw) {
                    break;
                }
            }
            Err(e) if raw.is_empty() => return Err(e.to_string()),
            // What arrived is parsed (a server that keeps the connection).
            Err(_) => break,
        }
        if Instant::now() >= deadline {
            if raw.is_empty() {
                return Err("délai dépassé".into());
            }
            break;
        }
    }
    parse(&raw)
}

/// The whole response is there (its `Content-Length` reached), so there
/// is no need to wait for the server to close the connection.
fn complete(raw: &[u8]) -> bool {
    let Some(split) = raw.windows(4).position(|w| w == b"\r\n\r\n") else {
        return false;
    };
    let head = String::from_utf8_lossy(&raw[..split]);
    head.lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse::<usize>().ok())
                .flatten()
        })
        .is_some_and(|len| raw.len() >= split + 4 + len)
}

/// Status and body of a raw HTTP response (chunked bodies decoded).
fn parse(raw: &[u8]) -> Result<Response, String> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| "réponse HTTP invalide".to_owned())?;
    let head = String::from_utf8_lossy(&raw[..split]);
    let mut lines = head.lines();
    let status_line = lines.next().unwrap_or_default();
    if !status_line.starts_with("HTTP/") {
        return Err("réponse HTTP invalide".into());
    }
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse::<u16>().ok())
        .ok_or_else(|| "réponse HTTP invalide".to_owned())?;
    let chunked = lines.any(|l| {
        l.split_once(':').is_some_and(|(k, v)| {
            k.trim().eq_ignore_ascii_case("transfer-encoding")
                && v.trim().eq_ignore_ascii_case("chunked")
        })
    });
    let mut body = &raw[split + 4..];
    let decoded;
    if chunked {
        let mut out = Vec::new();
        while let Some(end) = body.windows(2).position(|w| w == b"\r\n") {
            let size_text = String::from_utf8_lossy(&body[..end]);
            let size_text = size_text.split(';').next().unwrap_or_default().trim();
            let Ok(size) = usize::from_str_radix(size_text, 16) else {
                break;
            };
            if size == 0 || body.len() < end + 2 + size {
                break;
            }
            out.extend_from_slice(&body[end + 2..end + 2 + size]);
            body = &body[(end + 4 + size).min(body.len())..];
        }
        decoded = out;
        body = &decoded;
    }
    Ok(Response {
        status,
        body: String::from_utf8_lossy(body).into_owned(),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::net::TcpListener;

    /// A one-shot HTTP server on 127.0.0.1: returns its port; the request
    /// it received comes back through the join handle.
    pub(crate) fn serve_once(response: String) -> (u16, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let t = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_millis(500)))
                .unwrap();
            let mut req = Vec::new();
            let mut buf = [0u8; 2048];
            // Head, then the body announced by Content-Length.
            while let Ok(n) = s.read(&mut buf) {
                if n == 0 {
                    break;
                }
                req.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&req);
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let len = head
                        .lines()
                        .find_map(|l| l.strip_prefix("Content-Length: "))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if body.len() >= len {
                        break;
                    }
                }
            }
            let _ = s.write_all(response.as_bytes());
            String::from_utf8_lossy(&req).into_owned()
        });
        (port, t)
    }

    pub(crate) fn json_response(status: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
    }

    #[test]
    fn posts_json_and_reads_the_answer() {
        let (port, server) = serve_once(json_response("200 OK", r#"{"auth_token":"abc"}"#));
        let r = request(
            &format!("127.0.0.1:{port}"),
            80,
            "POST",
            "/api/v1/new",
            Some("{}"),
            Duration::from_secs(2),
        )
        .unwrap();
        assert!(r.ok());
        assert_eq!(r.json().unwrap()["auth_token"], "abc");
        let req = server.join().unwrap();
        assert!(req.starts_with("POST /api/v1/new HTTP/1.0\r\n"));
        assert!(req.contains("Content-Length: 2\r\n"));
        assert!(req.ends_with("\r\n\r\n{}"));
    }

    #[test]
    fn chunked_bodies_are_decoded() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"a\"\r\n3\r\n:1}\r\n0\r\n\r\n";
        let r = parse(raw).unwrap();
        assert_eq!(r.body, r#"{"a":1}"#);
    }

    #[test]
    fn targets_are_understood() {
        let (a, h) = target("192.168.1.40", 9123).unwrap();
        assert_eq!(a, "192.168.1.40:9123".parse().unwrap());
        assert_eq!(h, "192.168.1.40");
        let (a, _) = target("192.168.1.40:8080", 80).unwrap();
        assert_eq!(a.port(), 8080);
        let (a, _) = target("fe80::1", 80).unwrap();
        assert_eq!(a.port(), 80);
        let (a, _) = target("[fe80::1]:16021", 80).unwrap();
        assert_eq!(a.port(), 16021);
        assert!(target("192.168.1.40:port", 80).is_err());
    }
}
