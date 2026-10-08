//! Startup, liveness and readiness probes, the way Kubernetes has them
//! (stormd#48): same fields, same meaning.
//!
//! - The **startup** probe gates the other two: they do not run until it
//!   has succeeded once. Its allowance is `failure_threshold ×
//!   period_seconds`, so a slow first start (fastetcd opening its data, the
//!   apiserver reconciling manifests) is never killed.
//! - **Liveness** failing `failure_threshold` times in a row stops the run
//!   (SIGTERM, SIGKILL after `stop_timeout_secs`); the restart policy takes
//!   the exit.
//! - **Readiness** marks the process ready after `success_threshold` passes
//!   in a row and not ready after `failure_threshold` failures. It never
//!   restarts anything.
//!
//! **Why:** stormd's old liveness had no startup grace. On the Dell on
//! 2026-10-08 it killed the apiserver six times during a normal 40 s start
//! (liveness every 10 s, threshold 3), and fastetcd while it opened its
//! database (stormcos#186). The old `[process.liveness]` no longer kills.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// A probe, as in a Kubernetes container spec. Keys are snake_case; the
/// Kubernetes camelCase spellings are accepted too.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Probe {
    #[serde(default, alias = "httpGet")]
    pub http_get: Option<HttpGet>,
    #[serde(default, alias = "tcpSocket")]
    pub tcp_socket: Option<TcpSocket>,
    #[serde(default)]
    pub exec: Option<Exec>,
    #[serde(default)]
    pub grpc: Option<Grpc>,
    #[serde(default, alias = "initialDelaySeconds")]
    pub initial_delay_seconds: u64,
    #[serde(default = "d_period", alias = "periodSeconds")]
    pub period_seconds: u64,
    #[serde(default = "d_timeout", alias = "timeoutSeconds")]
    pub timeout_seconds: u64,
    #[serde(default = "d_one", alias = "successThreshold")]
    pub success_threshold: u32,
    #[serde(default = "d_three", alias = "failureThreshold")]
    pub failure_threshold: u32,
}

fn d_period() -> u64 { 10 }
fn d_timeout() -> u64 { 1 }
fn d_one() -> u32 { 1 }
fn d_three() -> u32 { 3 }

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct HttpGet {
    #[serde(default = "d_path")]
    pub path: String,
    pub port: u16,
    #[serde(default = "d_host")]
    pub host: String,
    /// `HTTP` or `HTTPS` (certificates are not verified, as upstream).
    #[serde(default = "d_scheme")]
    pub scheme: String,
    #[serde(default, alias = "httpHeaders")]
    pub http_headers: Vec<Header>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Header {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct TcpSocket {
    pub port: u16,
    #[serde(default = "d_host")]
    pub host: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Exec {
    pub command: Vec<String>,
}

/// `grpc.health.v1.Health/Check` on `port`, with an optional `service`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Grpc {
    pub port: u16,
    #[serde(default)]
    pub service: Option<String>,
}

fn d_path() -> String { "/".into() }
fn d_host() -> String { "127.0.0.1".into() }
fn d_scheme() -> String { "HTTP".into() }

impl Probe {
    /// What is wrong with it, if anything: exactly one action, sane numbers.
    /// `kind` is "startup", "liveness" or "readiness".
    pub fn check(&self, kind: &str) -> Result<(), String> {
        let actions = [self.http_get.is_some(), self.tcp_socket.is_some(), self.exec.is_some(), self.grpc.is_some()]
            .iter()
            .filter(|b| **b)
            .count();
        if actions != 1 {
            return Err("needs exactly one of http_get, tcp_socket, exec, grpc".into());
        }
        if self.period_seconds == 0 || self.timeout_seconds == 0 || self.failure_threshold == 0 || self.success_threshold == 0 {
            return Err("period_seconds, timeout_seconds and the thresholds must be at least 1".into());
        }
        if kind != "readiness" && self.success_threshold != 1 {
            return Err(format!("success_threshold must be 1 for a {kind} probe (as in Kubernetes)"));
        }
        if let Some(e) = &self.exec {
            if e.command.is_empty() {
                return Err("exec.command is empty".into());
            }
        }
        if let Some(h) = &self.http_get {
            if !h.scheme.eq_ignore_ascii_case("http") && !h.scheme.eq_ignore_ascii_case("https") {
                return Err(format!("http_get.scheme {:?} is not HTTP or HTTPS", h.scheme));
            }
        }
        Ok(())
    }

    /// One try. `Err` carries the message Kubernetes would put in an
    /// `Unhealthy` event.
    pub async fn run(&self) -> Result<(), String> {
        let timeout = Duration::from_secs(self.timeout_seconds.max(1));
        if let Some(h) = &self.http_get {
            let scheme = if h.scheme.eq_ignore_ascii_case("https") { "https" } else { "http" };
            let path = if h.path.starts_with('/') { h.path.clone() } else { format!("/{}", h.path) };
            let url = format!("{scheme}://{}:{}{path}", h.host, h.port);
            let client = reqwest::Client::builder()
                .timeout(timeout)
                .danger_accept_invalid_certs(true)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|e| e.to_string())?;
            let mut req = client.get(&url);
            for hd in &h.http_headers {
                req = req.header(&hd.name, &hd.value);
            }
            return match req.send().await {
                Ok(r) if r.status().as_u16() >= 200 && r.status().as_u16() < 400 => Ok(()),
                Ok(r) => Err(format!("HTTP probe failed with statuscode: {}", r.status().as_u16())),
                Err(e) if e.is_timeout() => Err(format!("Get \"{url}\": context deadline exceeded")),
                Err(e) => Err(format!("Get \"{url}\": {e}")),
            };
        }
        if let Some(t) = &self.tcp_socket {
            let addr = format!("{}:{}", t.host, t.port);
            return match tokio::time::timeout(timeout, tokio::net::TcpStream::connect(&addr)).await {
                Ok(Ok(_)) => Ok(()),
                Ok(Err(e)) => Err(format!("dial tcp {addr}: {e}")),
                Err(_) => Err(format!("dial tcp {addr}: i/o timeout")),
            };
        }
        if let Some(x) = &self.exec {
            let mut cmd = tokio::process::Command::new(&x.command[0]);
            cmd.args(&x.command[1..])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true);
            return match tokio::time::timeout(timeout, cmd.output()).await {
                Ok(Ok(o)) if o.status.success() => Ok(()),
                Ok(Ok(o)) => {
                    let mut out = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    let err = String::from_utf8_lossy(&o.stderr);
                    if !err.trim().is_empty() {
                        out = format!("{out} {}", err.trim()).trim().to_string();
                    }
                    Err(format!("command {:?} exited with {}: {out}", x.command, o.status.code().map(|c| c.to_string()).unwrap_or("a signal".into())))
                }
                Ok(Err(e)) => Err(format!("command {:?}: {e}", x.command)),
                Err(_) => Err(format!("command {:?} timed out after {}s", x.command, self.timeout_seconds)),
            };
        }
        if let Some(g) = &self.grpc {
            return grpc_check(g, timeout).await;
        }
        Err("probe has no action".into())
    }
}

/// `grpc.health.v1.Health/Check` over plaintext HTTP/2, as the kubelet's
/// gRPC probe does: passes only on `SERVING`. A TCP connect would pass while
/// the server is up but not serving.
async fn grpc_check(g: &Grpc, timeout: Duration) -> Result<(), String> {
    let addr = format!("127.0.0.1:{}", g.port);
    let client = reqwest::Client::builder()
        .http2_prior_knowledge()
        .timeout(timeout)
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .post(format!("http://{addr}/grpc.health.v1.Health/Check"))
        .header("content-type", "application/grpc")
        .header("te", "trailers")
        .body(grpc_request(g.service.as_deref().unwrap_or("")))
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                format!("timeout: failed to connect service \"{addr}\" within {}s", timeout.as_secs())
            } else {
                format!("failed to connect service \"{addr}\": {e}")
            }
        })?;
    // An error comes trailers-only: grpc-status in the headers.
    if let Some(st) = resp.headers().get("grpc-status").and_then(|v| v.to_str().ok()) {
        if st != "0" {
            let msg = resp.headers().get("grpc-message").and_then(|v| v.to_str().ok()).unwrap_or("");
            return Err(format!("rpc error: code = {st} desc = {msg}"));
        }
    }
    let body = resp.bytes().await.map_err(|e| e.to_string())?;
    match grpc_serving_status(&body) {
        Some(1) => Ok(()),
        Some(s) => Err(format!("service unhealthy (responded with \"{}\")", serving_name(s))),
        None => Err("service unhealthy (no health response)".into()),
    }
}

/// A length-prefixed `HealthCheckRequest { service }`.
fn grpc_request(service: &str) -> Vec<u8> {
    let mut msg = Vec::new();
    if !service.is_empty() {
        msg.push(0x0a); // field 1, length-delimited
        put_varint(&mut msg, service.len() as u64);
        msg.extend_from_slice(service.as_bytes());
    }
    let mut out = vec![0u8];
    out.extend_from_slice(&(msg.len() as u32).to_be_bytes());
    out.extend(msg);
    out
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// The `status` of a length-prefixed `HealthCheckResponse`: 0 UNKNOWN when
/// absent (proto3 default), `None` when the frame is not one.
fn grpc_serving_status(frame: &[u8]) -> Option<u64> {
    if frame.len() < 5 || frame[0] != 0 {
        return None;
    }
    let len = u32::from_be_bytes([frame[1], frame[2], frame[3], frame[4]]) as usize;
    let msg = frame.get(5..5 + len)?;
    let mut i = 0;
    let mut status = 0;
    while i < msg.len() {
        let (tag, n) = varint(&msg[i..])?;
        i += n;
        match tag & 7 {
            0 => {
                let (v, n) = varint(&msg[i..])?;
                i += n;
                if tag >> 3 == 1 {
                    status = v;
                }
            }
            2 => {
                let (l, n) = varint(&msg[i..])?;
                i += n + l as usize;
            }
            _ => return None,
        }
    }
    Some(status)
}

fn varint(b: &[u8]) -> Option<(u64, usize)> {
    let mut v = 0u64;
    for (i, byte) in b.iter().enumerate().take(10) {
        v |= ((byte & 0x7f) as u64) << (7 * i);
        if byte & 0x80 == 0 {
            return Some((v, i + 1));
        }
    }
    None
}

fn serving_name(s: u64) -> &'static str {
    match s {
        0 => "UNKNOWN",
        1 => "SERVING",
        2 => "NOT_SERVING",
        3 => "SERVICE_UNKNOWN",
        _ => "?",
    }
}

/// Consecutive results of one probe, and what they add up to.
#[derive(Debug, Default, Clone)]
pub struct Counter {
    pub successes: u32,
    pub failures: u32,
}

impl Counter {
    /// Record a result. Returns `Some(true)` when the success threshold is
    /// reached, `Some(false)` when the failure threshold is, `None` otherwise.
    pub fn record(&mut self, ok: bool, p: &Probe) -> Option<bool> {
        if ok {
            self.failures = 0;
            self.successes += 1;
            (self.successes >= p.success_threshold).then_some(true)
        } else {
            self.successes = 0;
            self.failures += 1;
            (self.failures >= p.failure_threshold).then_some(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(t: &str) -> Probe {
        toml::from_str(t).unwrap()
    }

    #[test]
    fn kubernetes_defaults_and_spellings() {
        let a = p("tcp_socket = { port = 2379 }\n");
        assert_eq!((a.initial_delay_seconds, a.period_seconds, a.timeout_seconds, a.success_threshold, a.failure_threshold), (0, 10, 1, 1, 3));
        let b = p("httpGet = { path = \"/healthz\", port = 6443, scheme = \"HTTPS\" }\nperiodSeconds = 2\nfailureThreshold = 150\n");
        assert_eq!((b.period_seconds, b.failure_threshold, b.http_get.as_ref().unwrap().port), (2, 150, 6443));
        assert!(b.check("startup").is_ok());
        assert!(p("exec = { command = [] }\n").check("liveness").is_err());
        assert!(p("tcp_socket = { port = 1 }\nexec = { command = [\"x\"] }\n").check("liveness").is_err());
        assert!(p("tcp_socket = { port = 1 }\nsuccess_threshold = 2\n").check("liveness").is_err());
        assert!(p("tcp_socket = { port = 1 }\nsuccess_threshold = 2\n").check("readiness").is_ok());
    }

    #[test]
    fn grpc_frames() {
        assert_eq!(grpc_request(""), vec![0, 0, 0, 0, 0]);
        assert_eq!(grpc_request("etcd"), vec![0, 0, 0, 0, 6, 0x0a, 4, b'e', b't', b'c', b'd']);
        assert_eq!(grpc_serving_status(&[0, 0, 0, 0, 2, 0x08, 1]), Some(1));
        assert_eq!(grpc_serving_status(&[0, 0, 0, 0, 2, 0x08, 2]), Some(2));
        assert_eq!(grpc_serving_status(&[0, 0, 0, 0, 0]), Some(0), "absent = UNKNOWN");
        assert_eq!(grpc_serving_status(&[1, 0, 0, 0, 0]), None, "compressed: not understood");
        assert_eq!(grpc_serving_status(&[0, 0]), None);
    }

    #[tokio::test]
    async fn grpc_to_a_closed_port_fails() {
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let e = p(&format!("grpc = {{ port = {closed} }}\n")).run().await.unwrap_err();
        assert!(e.contains("failed to connect service"), "{e}");
    }

    #[test]
    fn counters_need_consecutive_results() {
        let probe = p("tcp_socket = { port = 1 }\nfailure_threshold = 3\nsuccess_threshold = 2\n");
        let mut c = Counter::default();
        assert_eq!(c.record(false, &probe), None);
        assert_eq!(c.record(false, &probe), None);
        assert_eq!(c.record(true, &probe), None, "a success resets the failures");
        assert_eq!(c.record(false, &probe), None);
        assert_eq!(c.record(false, &probe), None);
        assert_eq!(c.record(false, &probe), Some(false));
        assert_eq!(c.record(true, &probe), None);
        assert_eq!(c.record(true, &probe), Some(true));
    }

    #[tokio::test]
    async fn actions_pass_and_fail_with_kubernetes_messages() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move { loop { let _ = l.accept().await; } });
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert!(p(&format!("tcp_socket = {{ port = {port} }}\n")).run().await.is_ok());
        let e = p(&format!("tcp_socket = {{ port = {closed} }}\n")).run().await.unwrap_err();
        assert!(e.starts_with("dial tcp 127.0.0.1:"), "{e}");
        assert!(p("exec = { command = [\"/bin/true\"] }\n").run().await.is_ok());
        let e = p("exec = { command = [\"/bin/sh\", \"-c\", \"echo nope; exit 3\"] }\n").run().await.unwrap_err();
        assert!(e.contains("exited with 3") && e.contains("nope"), "{e}");
        let e = p(&format!("http_get = {{ port = {closed} }}\n")).run().await.unwrap_err();
        assert!(e.starts_with("Get \"http://127.0.0.1:"), "{e}");
    }
}
