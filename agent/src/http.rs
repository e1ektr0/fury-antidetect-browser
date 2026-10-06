// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! The local automation API.
//!
//! Every competitor ships one on a fixed loopback port — AdsPower :50325,
//! Dolphin :3001, Octo :58888 — because it is what makes the product
//! scriptable from any language that can make an HTTP request. A Unix socket
//! speaking newline-delimited JSON is fine for the shell that ships beside it
//! and useless to a customer's Python.
//!
//! # Why this is dangerous, and what stops it
//!
//! `ipc.rs` says plainly why the shell uses a socket: on a machine where a
//! browser runs arbitrary sites' JavaScript all day, `127.0.0.1` is reachable
//! from a page. A profile visiting a hostile site could `fetch()` this port and
//! read the organisation's proxy list. Opening it without answering that would
//! undo the reason the socket was chosen.
//!
//! Four things answer it, and each is load-bearing:
//!
//! 1. **A bearer token**, generated on first run into a 0600 file. A page
//!    cannot read that file, so it cannot form an authorised request.
//! 2. **Any request carrying `Origin` is refused.** A browser attaches that
//!    header to every cross-origin `fetch` and `XMLHttpRequest`; a script does
//!    not. This is what actually keeps pages out, token or no token, and it
//!    holds even if a token leaks into a page's reach.
//! 3. **`Host` must be loopback.** Otherwise a name that resolves to 127.0.0.1
//!    lets a page treat this as same-origin — DNS rebinding, which defeats (2)
//!    by removing the cross-origin-ness.
//! 4. **Bound to 127.0.0.1**, so nothing off the machine can reach it at all.
//!
//! Off unless switched on. A port that exists because it shipped enabled is a
//! port on the machines of everyone who never wanted it.

use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// The conventional port for the automation API.
///
/// Nothing binds it by default — the API is off until `FURY_API_PORT` says
/// otherwise ([`crate::api_port`]). This is the number the examples and the
/// documentation use, so that scripts agree with each other.
///
/// Not one of the competitors' numbers: a script pointed at the wrong product
/// should fail to connect rather than half-work against it.
pub const DEFAULT_PORT: u16 = 35000;
const MAX_HEAD: usize = 16 * 1024;
const MAX_BODY: usize = 1024 * 1024;
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Where the token lives. Mode 0600, beside the database.
pub fn token_path() -> std::path::PathBuf {
    crate::paths::data_dir().join("api-token")
}

/// The token, created on first use.
pub fn token() -> anyhow::Result<String> {
    let path = token_path();
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let trimmed = existing.trim().to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }

    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();

    std::fs::write(&path, &token)?;
    // The whole authorisation story, in one file — so it is restricted to this
    // user explicitly rather than left to inherit whatever the directory had.
    fury_platform::perms::owner_only_file(&path)?;
    Ok(token)
}

pub async fn bind(port: u16) -> anyhow::Result<(TcpListener, String)> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    let token = token()?;
    tracing::info!(
        port,
        token = %token_path().display(),
        "local automation API listening"
    );
    Ok((listener, token))
}

pub async fn serve(agent: Arc<crate::ipc::Agent>, listener: TcpListener, token: String) -> anyhow::Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        let agent = Arc::clone(&agent);
        let token = token.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(stream, agent, token).await {
                tracing::debug!(error = %e, "api connection ended");
            }
        });
    }
}

struct Request {
    method: String,
    path: String,
    body: String,
    origin: Option<String>,
    host: Option<String>,
    auth: Option<String>,
}

async fn handle(
    mut stream: TcpStream,
    agent: Arc<crate::ipc::Agent>,
    token: String,
) -> anyhow::Result<()> {
    let req = match tokio::time::timeout(READ_TIMEOUT, read_request(&mut stream)).await {
        Ok(Ok(Some(req))) => req,
        Ok(Ok(None)) => return Ok(()),
        Ok(Err(e)) => return respond(&mut stream, 400, &serde_json::json!({
            "error":"invalid_request", "message": e.to_string()
        })).await,
        Err(_) => return respond(&mut stream, 408, &serde_json::json!({
            "error":"request_timeout", "message":"request was not received within five seconds"
        })).await,
    };

    // (2) and (3) before anything else, and before the token is even compared:
    // a page must not be able to learn whether its guess was right.
    if req.origin.is_some() {
        return respond(
            &mut stream,
            403,
            &serde_json::json!({
                "error": "origin_present",
                "message": "This endpoint refuses requests from a browser. It is for scripts.",
            }),
        )
        .await;
    }
    let host_ok = req
        .host
        .as_deref()
        .map(|h| {
            let name = h.rsplit_once(':').map(|(n, _)| n).unwrap_or(h);
            name == "127.0.0.1" || name == "localhost" || name == "[::1]"
        })
        .unwrap_or(false);
    if !host_ok {
        return respond(
            &mut stream,
            403,
            &serde_json::json!({
                "error": "bad_host",
                "message": "Address this by its loopback address.",
            }),
        )
        .await;
    }

    let presented = req
        .auth
        .as_deref()
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    // Constant time, because the comparison is against a secret and the
    // attacker controls one side of it.
    if !constant_time_eq(presented.as_bytes(), token.as_bytes()) {
        return respond(
            &mut stream,
            401,
            &serde_json::json!({
                "error": "unauthenticated",
                "message": "Send the token from the api-token file as a bearer token.",
            }),
        )
        .await;
    }

    let params: serde_json::Value = if req.body.is_empty() {
        serde_json::json!({})
    } else {
        match serde_json::from_str(&req.body) {
            Ok(value) => value,
            Err(_) => return respond(&mut stream, 400, &serde_json::json!({
                "error":"invalid_json", "message":"request body is not valid JSON"
            })).await,
        }
    };
    let params = if params.is_null() {
        serde_json::json!({})
    } else {
        params
    };

    let profile_id = req.path.strip_prefix("/v1/profiles/")
        .filter(|id| !id.is_empty() && !id.contains('/'));
    let proxy_id = req.path.strip_prefix("/v1/proxies/")
        .filter(|id| !id.is_empty() && !id.contains('/'));
    let management = match (req.method.as_str(), req.path.as_str()) {
        ("POST", "/v1/profiles") => Some(agent.create_profile(params.clone()).await),
        ("POST", "/v1/proxies") => Some(agent.save_proxy(None, params.clone()).await),
        ("GET", _) if profile_id.is_some() => Some(agent.get_profile(profile_id.unwrap()).await),
        ("PUT", _) if profile_id.is_some() => Some(agent.update_profile(profile_id.unwrap(), params.clone()).await),
        ("DELETE", _) if profile_id.is_some() => Some(agent.delete_profile(profile_id.unwrap()).await),
        ("PUT", _) if proxy_id.is_some() => Some(agent.save_proxy(proxy_id, params.clone()).await),
        _ => None,
    };
    if let Some(result) = management {
        return match result {
            Ok(value) => respond(&mut stream, 200, &serde_json::json!({"ok": true, "data": value})).await,
            Err(e) => {
                let (status, code) = e.downcast_ref::<crate::ipc::ManagementError>()
                    .map(|e| (e.status, e.code)).unwrap_or((500, "internal_error"));
                respond(&mut stream, status, &serde_json::json!({"error": code, "message": e.to_string()})).await
            }
        };
    }

    // Thin on purpose: every route is one IPC method, so the HTTP surface
    // cannot drift away from what the shell uses and be separately wrong.
    let route = match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/v1/status") => Some(("status", params)),
        ("GET", "/v1/profiles") => Some(("profiles.list", params)),
        ("GET", "/v1/proxies") => Some(("proxies.list", params)),
        ("GET", "/v1/personas") => Some(("personas.list", params)),
        ("POST", "/v1/profiles/start") => Some(("profile.launch", params)),
        ("POST", "/v1/profiles/stop") => Some(("profile.stop", params)),
        _ => None,
    };

    let Some((method, params)) = route else {
        return respond(
            &mut stream,
            404,
            &serde_json::json!({ "error": "not_found", "message": "No such endpoint." }),
        )
        .await;
    };

    match agent.dispatch_public(method, params).await {
        Ok(value) => respond(&mut stream, 200, &serde_json::json!({ "ok": true, "data": value })).await,
        Err(e) => {
            respond(
                &mut stream,
                400,
                &serde_json::json!({ "error": "failed", "message": e.to_string() }),
            )
            .await
        }
    }
}

async fn read_request(stream: &mut TcpStream) -> anyhow::Result<Option<Request>> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];

    // Headers first. Bounded, because an unbounded read from an unauthenticated
    // socket is a way to exhaust this process's memory from a page that cannot
    // even authenticate.
    let head_end = loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            if buf.is_empty() { return Ok(None); }
            anyhow::bail!("incomplete request headers");
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find(&buf, b"\r\n\r\n") {
            if pos + 4 > MAX_HEAD { anyhow::bail!("request head too large"); }
            break pos + 4;
        }
        if buf.len() > MAX_HEAD {
            anyhow::bail!("request head too large");
        }
    };

    let head = std::str::from_utf8(&buf[..head_end])?;
    let mut lines = head.lines();
    let start = lines.next().unwrap_or_default();
    let parts: Vec<_> = start.split_whitespace().collect();
    if parts.len() != 3 || !["HTTP/1.0", "HTTP/1.1"].contains(&parts[2])
        || !parts[1].starts_with('/') || parts[1].starts_with("//") {
        anyhow::bail!("invalid request line");
    }
    let method = parts[0].to_string();
    let target = parts[1];
    let path = target.split('?').next().unwrap_or("").to_string();
    let mut headers = std::collections::HashMap::new();
    for line in lines.filter(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').ok_or_else(|| anyhow::anyhow!("malformed header"))?;
        if name.is_empty() || name.bytes().any(|b| !b.is_ascii_alphanumeric() && b != b'-') {
            anyhow::bail!("invalid header name");
        }
        let name = name.to_ascii_lowercase();
        if headers.insert(name.clone(), value.trim().to_string()).is_some()
            && ["host", "origin", "authorization", "content-length", "transfer-encoding"].contains(&name.as_str()) {
            anyhow::bail!("duplicate sensitive or framing header");
        }
    }
    if headers.contains_key("transfer-encoding") { anyhow::bail!("transfer encoding is not supported"); }
    let length: usize = match headers.get("content-length") {
        Some(value) if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) => value.parse()?,
        Some(_) => anyhow::bail!("invalid content length"),
        None => 0,
    };
    if length > MAX_BODY {
        anyhow::bail!("request body too large");
    }

    let mut body = buf[head_end..].to_vec();
    if body.len() > length { anyhow::bail!("body exceeds content length; pipelining is not supported"); }
    while body.len() < length {
        let remaining = (length - body.len()).min(chunk.len());
        let n = stream.read(&mut chunk[..remaining]).await?;
        if n == 0 {
            anyhow::bail!("incomplete request body");
        }
        body.extend_from_slice(&chunk[..n]);
    }

    Ok(Some(Request {
        method,
        path,
        body: String::from_utf8(body)?,
        origin: headers.remove("origin"),
        host: headers.remove("host"),
        auth: headers.remove("authorization"),
    }))
}

async fn respond(
    stream: &mut TcpStream,
    status: u16,
    body: &serde_json::Value,
) -> anyhow::Result<()> {
    let body = serde_json::to_vec(body)?;
    let head = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n",
        match status {
            200 => "OK",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            409 => "Conflict",
            408 => "Request Timeout",
            500 => "Internal Server Error",
            503 => "Service Unavailable",
            _ => "Bad Request",
        },
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(&body).await?;
    stream.flush().await?;
    Ok(())
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn occupied_api_port_is_a_startup_error() {
        let occupied = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        assert!(bind(occupied.local_addr().unwrap().port()).await.is_err());
    }

    async fn raw(url: &str, request: &str, eof: bool) -> String {
        let mut socket = TcpStream::connect(url.strip_prefix("http://").unwrap()).await.unwrap();
        socket.write_all(request.as_bytes()).await.unwrap();
        if eof { socket.shutdown().await.unwrap(); }
        let mut response = Vec::new();
        socket.read_to_end(&mut response).await.unwrap();
        String::from_utf8(response).unwrap()
    }

    #[tokio::test]
    async fn malformed_requests_never_mutate_and_authorization_cannot_be_bypassed() {
        let dir = crate::tmp::TempDir::new("http-input");
        let agent = crate::ipc::Agent::for_tests(&dir.join("test.db")).await.unwrap();
        let (url, task) = server(agent.clone()).await;
        let head = "POST /v1/profiles HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer test-token\r\n";
        for tail in [
            "Content-Length: 1\r\n\r\n{",
            "Content-Length: 10\r\n\r\n{}",
            "Content-Length: abc\r\n\r\n",
            "Content-Length: 1048577\r\n\r\n",
            "Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}",
            "Transfer-Encoding: chunked\r\nContent-Length: 2\r\n\r\n{}",
            "Authorization: Bearer other\r\n\r\n",
            "Host: attacker.example\r\n\r\n",
        ] {
            assert!(raw(&url, &format!("{head}{tail}"), true).await.starts_with("HTTP/1.1 400"));
        }
        let oversized = format!("{head}X-Padding: {}\r\n\r\n", "a".repeat(MAX_HEAD));
        assert!(raw(&url, &oversized, true).await.starts_with("HTTP/1.1 400"));
        let origin = format!("{head}Origin: https://example.com\r\n\r\n");
        assert!(raw(&url, &origin, true).await.starts_with("HTTP/1.1 403"));
        let wrong_host = "GET /v1/profiles HTTP/1.1\r\nHost: attacker.example\r\nAuthorization: Bearer test-token\r\n\r\n";
        assert!(raw(&url, wrong_host, true).await.starts_with("HTTP/1.1 403"));
        let wrong_token = "GET /v1/profiles HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer wrong\r\n\r\n";
        assert!(raw(&url, wrong_token, true).await.starts_with("HTTP/1.1 401"));
        let partial = format!("{head}Content-Length: 10\r\n\r\n{{}}");
        assert!(raw(&url, &partial, false).await.starts_with("HTTP/1.1 408"));
        assert_eq!(agent.dispatch_public("profiles.list", serde_json::json!({})).await.unwrap(), serde_json::json!([]));
        task.abort();
    }

    async fn server(agent: Arc<crate::ipc::Agent>) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let agent = agent.clone();
                tokio::spawn(async move { handle(stream, agent, "test-token".into()).await.unwrap(); });
            }
        });
        (url, task)
    }

    #[tokio::test]
    async fn management_routes_preserve_legacy_surface_and_report_client_errors() {
        use serde_json::json;
        let dir = crate::tmp::TempDir::new("http-management");
        let agent = crate::ipc::Agent::for_tests(&dir.join("test.db")).await.unwrap();
        let (url, task) = server(agent.clone()).await;
        let client = reqwest::Client::new();
        let input = json!({"name":"API profile", "persona_id": fury_shared::catalogue::all()[0].id});
        let unauthorized = client.get(format!("{url}/v1/profiles")).send().await.unwrap();
        assert_eq!(unauthorized.status(), 401);
        let created: serde_json::Value = client.post(format!("{url}/v1/profiles"))
            .bearer_auth("test-token").json(&input).send().await.unwrap().json().await.unwrap();
        let id = created["data"]["id"].as_str().unwrap();
        assert_eq!(created["ok"], true);
        let proxy_input = json!({"name":"API exit", "kind":"socks5", "host":"localhost", "port":1080});
        let proxy: serde_json::Value = client.post(format!("{url}/v1/proxies"))
            .bearer_auth("test-token").json(&proxy_input).send().await.unwrap().json().await.unwrap();
        let proxy_id = proxy["data"]["id"].as_str().unwrap();
        assert_eq!(client.put(format!("{url}/v1/proxies/{proxy_id}"))
            .bearer_auth("test-token").json(&proxy_input).send().await.unwrap().status(), 200);
        let fetched: serde_json::Value = client.get(format!("{url}/v1/profiles/{id}"))
            .bearer_auth("test-token").send().await.unwrap().json().await.unwrap();
        assert_eq!(fetched["data"]["name"], "API profile");
        let mut changed = input.clone();
        changed["name"] = json!("Updated");
        assert_eq!(client.put(format!("{url}/v1/profiles/{id}"))
            .bearer_auth("test-token").json(&changed).send().await.unwrap().status(), 200);
        assert_eq!(client.put(format!("{url}/v1/profiles/missing"))
            .bearer_auth("test-token").json(&input).send().await.unwrap().status(), 404);
        assert_eq!(client.post(format!("{url}/v1/profiles"))
            .bearer_auth("test-token").json(&json!({"name":"bad", "persona_id":"missing"}))
            .send().await.unwrap().status(), 400);
        for route in ["status", "profiles", "proxies", "personas"] {
            assert_eq!(client.get(format!("{url}/v1/{route}"))
                .bearer_auth("test-token").send().await.unwrap().status(), 200);
        }
        let stop: serde_json::Value = client.post(format!("{url}/v1/profiles/stop"))
            .bearer_auth("test-token").json(&json!({"id":id})).send().await.unwrap().json().await.unwrap();
        assert_eq!(stop["data"]["stopped"], false);
        agent.test_running(id).await;
        let conflict = client.delete(format!("{url}/v1/profiles/{id}"))
            .bearer_auth("test-token").send().await.unwrap().status();
        agent.remove_test_running(id).await;
        assert_eq!(conflict, 409);
        assert_eq!(client.delete(format!("{url}/v1/profiles/{id}"))
            .bearer_auth("test-token").send().await.unwrap().status(), 200);
        assert_eq!(client.get(format!("{url}/v1/profiles/{id}"))
            .bearer_auth("test-token").send().await.unwrap().status(), 404);
        task.abort();
    }

    #[test]
    fn tokens_compare_without_leaking_where_they_differ() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }

    #[test]
    fn a_loopback_host_is_accepted_and_a_rebound_name_is_not() {
        // The check as `handle` performs it. A name that resolves to 127.0.0.1
        // would otherwise make this same-origin for a page, which is the whole
        // of DNS rebinding.
        let ok = |h: &str| {
            let name = h.rsplit_once(':').map(|(n, _)| n).unwrap_or(h);
            name == "127.0.0.1" || name == "localhost" || name == "[::1]"
        };
        assert!(ok("127.0.0.1:35000"));
        assert!(ok("localhost:35000"));
        assert!(ok("127.0.0.1"));
        assert!(!ok("api.attacker.example:35000"));
        assert!(!ok("fury.local:35000"));
    }
}
