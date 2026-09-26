//! One stormd under test: its config, its process, its REST API, and what it
//! leaves behind.
//!
//! Each instance gets its own directory and port, so tests are independent of
//! each other and of anything else on the node. Dropping an instance kills
//! stormd and anything it started that is still alive — the standard's
//! "cleans up on success and on failure".

use std::fs::File;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::env::Env;

pub struct Stormd {
    pub dir: PathBuf,
    pub port: u16,
    pub token: Option<String>,
    /// This binary, for `command = …` in process configs.
    pub me: String,
    child: Child,
    exited: Option<ExitStatus>,
}

/// A config's `[api] auth_token`, when a test wants authentication on.
#[derive(Default)]
pub struct Opts {
    pub token: Option<String>,
}

impl Stormd {
    /// Write a config — the common head plus `body` (processes, cron) — and
    /// start stormd on it. `body` may use `{me}` for this binary and `{dir}`
    /// for the instance directory.
    pub fn start(env: &Env, label: &str, body: &str, opts: Opts) -> Result<Stormd, String> {
        let dir = env.instance_dir(label).map_err(|e| format!("cannot create {label} dir: {e}"))?;
        std::fs::create_dir_all(dir.join("pids")).map_err(|e| e.to_string())?;
        let port = free_port()?;
        let me = env.me.display().to_string();
        let auth = opts.token.as_ref().map(|t| format!("auth_token = \"{t}\"\n")).unwrap_or_default();
        let config = format!(
            "[general]\nname = \"stormd-test-{label}\"\nlog_dir = \"{log}\"\n\n\
             [api]\nbind = \"127.0.0.1:{port}\"\n{auth}\n\
             [stormlog.mcast]\ngroup = \"off\"\n\n\
             [ssh]\nenabled = false\n\n{body}",
            log = dir.join("log").display(),
            body = body.replace("{me}", &me).replace("{dir}", &dir.display().to_string()),
        );
        std::fs::write(dir.join("config.toml"), &config).map_err(|e| e.to_string())?;
        Self::spawn(env, dir, port, me, opts.token)
    }

    /// Start stormd on a config file that is already written (a test of a
    /// config stormd must refuse).
    pub fn start_raw(env: &Env, label: &str, config: &str) -> Result<Stormd, String> {
        let dir = env.instance_dir(label).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("config.toml"), config).map_err(|e| e.to_string())?;
        Self::spawn(env, dir, 0, env.me.display().to_string(), None)
    }

    fn spawn(env: &Env, dir: PathBuf, port: u16, me: String, token: Option<String>) -> Result<Stormd, String> {
        if !env.stormd.exists() {
            return Err(format!("no stormd binary at {}", env.stormd.display()));
        }
        let out = File::create(dir.join("stormd.out")).map_err(|e| e.to_string())?;
        let err = out.try_clone().map_err(|e| e.to_string())?;
        let child = Command::new(&env.stormd)
            .arg("--config")
            .arg(dir.join("config.toml"))
            .env("HELPER_PIDS", dir.join("pids"))
            .env("RUST_LOG", "info")
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err)
            .spawn()
            .map_err(|e| format!("cannot run {}: {e}", env.stormd.display()))?;
        Ok(Stormd { dir, port, token, me, child, exited: None })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// stormd's own output so far (its log lines), for a failure's detail.
    pub fn output(&self) -> String {
        std::fs::read_to_string(self.dir.join("stormd.out")).unwrap_or_default()
    }

    /// The last `n` lines of stormd's output.
    pub fn tail(&self, n: usize) -> String {
        let out = self.output();
        let lines: Vec<&str> = out.lines().collect();
        lines[lines.len().saturating_sub(n)..].join(" | ")
    }

    /// Whether stormd has exited, and how.
    pub fn exit_status(&mut self) -> Option<ExitStatus> {
        if self.exited.is_none() {
            self.exited = self.child.try_wait().ok().flatten();
        }
        self.exited
    }

    /// Wait for the API to answer `/api/v1/health`.
    pub fn wait_healthy(&mut self, t: Duration) -> Result<Duration, String> {
        let start = Instant::now();
        while start.elapsed() < t {
            if let Some(s) = self.exit_status() {
                return Err(format!("stormd exited ({s}) before its API came up: {}", self.tail(6)));
            }
            if let Ok((200, _)) = self.get("/api/v1/health") {
                return Ok(start.elapsed());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err(format!("API not up on :{} after {} s: {}", self.port, t.as_secs(), self.tail(6)))
    }

    pub fn get(&self, path: &str) -> Result<(u16, String), String> {
        http(self.port, "GET", path, None, self.token.as_deref())
    }

    pub fn get_as(&self, path: &str, token: Option<&str>) -> Result<(u16, String), String> {
        http(self.port, "GET", path, None, token)
    }

    pub fn post(&self, path: &str, body: Option<&str>) -> Result<(u16, String), String> {
        http(self.port, "POST", path, body, self.token.as_deref())
    }

    /// `GET path` as JSON, requiring 200.
    pub fn json(&self, path: &str) -> Result<Value, String> {
        match self.get(path)? {
            (200, b) => serde_json::from_str(&b).map_err(|e| format!("{path}: not JSON ({e}): {b:.200}")),
            (s, b) => Err(format!("{path}: HTTP {s}: {b:.200}")),
        }
    }

    /// One process's status from the API.
    pub fn process(&self, name: &str) -> Result<Value, String> {
        self.json(&format!("/api/v1/processes/{name}"))
    }

    /// Poll `f` until it yields, or fail with what it last saw.
    pub fn wait_for<T>(
        &mut self,
        t: Duration,
        what: &str,
        mut f: impl FnMut(&Stormd) -> Result<Option<T>, String>,
    ) -> Result<T, String> {
        let start = Instant::now();
        let mut last = String::new();
        loop {
            match f(self) {
                Ok(Some(v)) => return Ok(v),
                Ok(None) => {}
                Err(e) => last = e,
            }
            if start.elapsed() >= t {
                return Err(format!("{what}: not after {} s ({last}); stormd: {}", t.as_secs(), self.tail(4)));
            }
            if let Some(s) = self.exit_status() {
                return Err(format!("{what}: stormd exited ({s}): {}", self.tail(6)));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Wait for a process to reach `state` (`running`, `stopped`, `failed`…).
    pub fn wait_state(&mut self, name: &str, state: &str, t: Duration) -> Result<Value, String> {
        self.wait_for(t, &format!("{name} {state}"), |s| {
            let p = s.process(name)?;
            Ok((p["state"] == state).then_some(p))
        })
    }

    /// Send a signal to stormd.
    pub fn signal(&self, sig: i32) {
        // SAFETY: kill(2) on a pid we own.
        unsafe {
            libc::kill(self.child.id() as i32, sig);
        }
    }

    /// Wait up to `t` for stormd to exit.
    pub fn wait_exit(&mut self, t: Duration) -> Option<(ExitStatus, Duration)> {
        let start = Instant::now();
        while start.elapsed() < t {
            if let Some(s) = self.exit_status() {
                return Some((s, start.elapsed()));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }

    /// SIGTERM, and how long stormd took to exit, with what code.
    pub fn terminate(&mut self, t: Duration) -> Result<(Option<i32>, Duration), String> {
        self.signal(libc::SIGTERM);
        match self.wait_exit(t) {
            Some((s, d)) => Ok((s.code(), d)),
            None => Err(format!("stormd still running {} s after SIGTERM: {}", t.as_secs(), self.tail(4))),
        }
    }

    /// Helpers this instance started that are still alive.
    pub fn leftover(&self) -> Vec<u32> {
        let Ok(rd) = std::fs::read_dir(self.dir.join("pids")) else {
            return Vec::new();
        };
        rd.flatten()
            .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
            .filter(|&pid| alive(pid))
            .collect()
    }

    /// Resident memory of stormd, in KiB.
    pub fn rss_kb(&self) -> Option<u64> {
        let s = std::fs::read_to_string(format!("/proc/{}/status", self.pid())).ok()?;
        s.lines()
            .find(|l| l.starts_with("VmRSS:"))?
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()
    }

    /// Open file descriptors of stormd.
    pub fn fds(&self) -> Option<usize> {
        Some(std::fs::read_dir(format!("/proc/{}/fd", self.pid())).ok()?.count())
    }
}

impl Drop for Stormd {
    fn drop(&mut self) {
        if self.exit_status().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        for pid in self.leftover() {
            // SAFETY: kill(2); the pid was recorded by a helper of ours.
            unsafe {
                libc::kill(pid as i32, libc::SIGKILL);
            }
        }
    }
}

/// Alive and not a zombie. A helper orphaned by stormd is reparented, and in
/// the image PID 1 is this binary, which does not reap it — a zombie has
/// stopped running, which is what the question is.
pub fn alive(pid: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(s) => {
            // "pid (comm) S …": the state follows the last ')'.
            let state = s.rsplit_once(')').and_then(|(_, r)| r.trim_start().chars().next());
            !matches!(state, Some('Z') | Some('X') | None)
        }
        Err(_) => false,
    }
}

/// A port nothing is listening on now. Another process could take it before
/// stormd binds; a test that loses that race says so in its detail.
pub fn free_port() -> Result<u16, String> {
    let l = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("cannot bind a local port: {e}"))?;
    l.local_addr().map(|a| a.port()).map_err(|e| e.to_string())
}

/// A minimal HTTP/1.1 client: one request per connection, 5 s timeouts.
pub fn http(port: u16, method: &str, path: &str, body: Option<&str>, token: Option<&str>) -> Result<(u16, String), String> {
    http_to(&format!("127.0.0.1:{port}"), method, path, body, token, Duration::from_secs(5))
}

pub fn http_to(
    addr: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
    token: Option<&str>,
    timeout: Duration,
) -> Result<(u16, String), String> {
    let sa = std::net::ToSocketAddrs::to_socket_addrs(addr)
        .map_err(|e| format!("{addr}: {e}"))?
        .next()
        .ok_or_else(|| format!("{addr}: no address"))?;
    let mut c = TcpStream::connect_timeout(&sa, timeout).map_err(|e| format!("{addr}: {e}"))?;
    let _ = c.set_read_timeout(Some(timeout));
    let _ = c.set_write_timeout(Some(timeout));
    let body = body.unwrap_or("");
    let auth = token.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
    let ctype = if body.is_empty() { "" } else { "Content-Type: application/json\r\n" };
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n{auth}{ctype}Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    c.write_all(req.as_bytes()).map_err(|e| format!("{addr}: {e}"))?;
    let mut raw = Vec::new();
    c.read_to_end(&mut raw).map_err(|e| format!("{addr}{path}: {e}"))?;
    parse_response(&raw).ok_or_else(|| format!("{addr}{path}: malformed response ({} bytes)", raw.len()))
}

fn parse_response(raw: &[u8]) -> Option<(u16, String)> {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&raw[..split]).ok()?;
    let status = head.split_whitespace().nth(1)?.parse().ok()?;
    let body = &raw[split + 4..];
    let chunked = head
        .lines()
        .any(|l| l.to_ascii_lowercase().starts_with("transfer-encoding:") && l.to_ascii_lowercase().contains("chunked"));
    let body = if chunked { dechunk(body)? } else { body.to_vec() };
    Some((status, String::from_utf8_lossy(&body).into_owned()))
}

fn dechunk(mut b: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let eol = b.windows(2).position(|w| w == b"\r\n")?;
        let size = usize::from_str_radix(std::str::from_utf8(&b[..eol]).ok()?.split(';').next()?.trim(), 16).ok()?;
        b = &b[eol + 2..];
        if size == 0 {
            return Some(out);
        }
        out.extend_from_slice(b.get(..size)?);
        b = b.get(size + 2..)?;
    }
}

#[cfg(test)]
mod tests {
    use super::parse_response;

    #[test]
    fn responses_parse_plain_and_chunked() {
        let r = parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok").unwrap();
        assert_eq!(r, (200, "ok".to_string()));
        let r = parse_response(b"HTTP/1.1 401 Unauthorized\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n").unwrap();
        assert_eq!(r, (401, "abcde".to_string()));
        assert!(parse_response(b"garbage").is_none());
    }
}
