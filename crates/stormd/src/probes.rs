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

/// gRPC health checks need HTTP/2, which this build does not carry yet:
/// say so rather than pass (stormd#48 follow-up).
async fn grpc_check(g: &Grpc, _timeout: Duration) -> Result<(), String> {
    Err(format!("grpc probe on port {} is not supported by this stormd yet", g.port))
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
