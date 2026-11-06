//! stormd#71, on the real binary: TLS configured and no pair at start. The
//! process that mints the pair is one of stormd's own, as on a node
//! (stormcert-serving-stormd under the stormcert container's stormd).

#[path = "../src/tls_fixtures.rs"]
#[allow(dead_code)]
mod tls_fixtures;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tls_fixtures::*;

struct Instance {
    dir: PathBuf,
    port: u16,
    child: Child,
}

impl Instance {
    /// Start stormd with `[api]` TLS over `<dir>/server.{crt,key}`, whatever
    /// `prepare` left there, supervising `processes`.
    fn start(label: &str, prepare: impl FnOnce(&Path), processes: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("stormd-71-{}-{}", label, uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        prepare(&dir);
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let d = dir.display();
        let config = format!(
            "[general]\nname = \"t71\"\nlog_dir = \"{d}/log\"\n\n\
             [api]\nbind = \"127.0.0.1:{port}\"\ntls_cert_file = \"{d}/server.crt\"\ntls_key_file = \"{d}/server.key\"\n\n\
             [stormlog.mcast]\ngroup = \"off\"\n\n[ssh]\nenabled = false\n\n{}",
            processes.replace("{dir}", &d.to_string()),
        );
        std::fs::write(dir.join("config.toml"), config).unwrap();
        let out = std::fs::File::create(dir.join("stormd.out")).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_stormd"))
            .arg("--config")
            .arg(dir.join("config.toml"))
            .env("RUST_LOG", "info")
            .stdin(Stdio::null())
            .stdout(out.try_clone().unwrap())
            .stderr(out)
            .spawn()
            .unwrap();
        Self { dir, port, child }
    }

    fn output(&self) -> String {
        std::fs::read_to_string(self.dir.join("stormd.out")).unwrap_or_default()
    }

    fn assert_alive(&mut self, when: &str) {
        if let Some(status) = self.child.try_wait().unwrap() {
            panic!("stormd exited {} ({}):\n{}", when, status, self.output());
        }
    }

    fn wait_for(&mut self, what: &str, mut check: impl FnMut(&Self) -> bool) {
        let start = Instant::now();
        while !check(self) {
            self.assert_alive(&format!("waiting for {what}"));
            assert!(start.elapsed() < Duration::from_secs(20), "no {} in 20 s:\n{}", what, self.output());
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Whatever a plain request gets back, as text; connection errors read
    /// as an empty answer. The listener must be up, or this proves nothing.
    fn plain_answer(&self) -> String {
        let mut c = std::net::TcpStream::connect(("127.0.0.1", self.port)).expect("the port is bound");
        c.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        c.write_all(b"GET /healthz HTTP/1.0\r\nHost: x\r\n\r\n").unwrap();
        let mut buf = Vec::new();
        let _ = c.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    }

    fn assert_no_plain_http(&self, when: &str) {
        let answer = self.plain_answer();
        assert!(!answer.contains("HTTP/"), "plain request answered {}: {:?}", when, answer);
    }

    async fn https(&self, path: &str) -> Result<reqwest::Response, reqwest::Error> {
        reqwest::Client::builder()
            .use_rustls_tls()
            .tls_built_in_root_certs(false)
            .add_root_certificate(reqwest::Certificate::from_pem(CA_CERT.as_bytes()).unwrap())
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap()
            .get(format!("https://127.0.0.1:{}{}", self.port, path))
            .send()
            .await
    }

    /// https never answers: the failure is the handshake, not a refused port.
    async fn assert_handshake_fails(&self, when: &str) {
        let r = self.https("/healthz").await;
        let err = r.as_ref().err().unwrap_or_else(|| panic!("https answered {}: {:?}", when, r));
        assert!(!err.is_timeout(), "https timed out {} rather than failing: {:?}", when, err);
        assert!(std::net::TcpStream::connect(("127.0.0.1", self.port)).is_ok(), "port not bound {}", when);
    }

    async fn assert_https_ok(&self) {
        let r = self.https("/healthz").await.unwrap();
        assert_eq!(r.status(), 200);
    }

    /// The process's run count and state, over the API.
    async fn process(&self, name: &str) -> serde_json::Value {
        self.https(&format!("/api/v1/processes/{}", name)).await.unwrap().json().await.unwrap()
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe { libc::kill(self.child.id() as i32, libc::SIGTERM) };
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(15) {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                let _ = std::fs::remove_dir_all(&self.dir);
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A shell process that drops `marker` and stays up.
fn resident(name: &str) -> String {
    format!(
        "[[process]]\nname = \"{name}\"\ncommand = \"/bin/sh\"\nargs = [\"-c\", \"touch {{dir}}/{name}.up; exec sleep 300\"]\n\n"
    )
}

/// The issue's first-boot case: the minter is stormd's own child and writes
/// the key first. Processes run, nothing answers; `go` lets the certificate
/// follow, and https answers from the same stormd.
#[tokio::test]
async fn the_stormd_that_mints_the_pair_starts_and_then_serves_it() {
    let mint = "[[process]]\nname = \"mint\"\ncommand = \"/bin/sh\"\non_exit = \"stop\"\n\
         args = [\"-c\", \"cp {dir}/src.key {dir}/server.key; while [ ! -e {dir}/go ]; do sleep 0.05; done; cp {dir}/src.crt {dir}/server.crt\"]\n\n";
    let mut sd = Instance::start(
        "mint",
        |dir| {
            std::fs::write(dir.join("src.key"), SERVER_KEY).unwrap();
            std::fs::write(dir.join("src.crt"), SERVER_CERT).unwrap();
        },
        &format!("{}{}", mint, resident("worker")),
    );
    let pid = sd.child.id();

    sd.wait_for("the mint's key", |s| s.dir.join("server.key").exists());
    sd.wait_for("the worker", |s| s.dir.join("worker.up").exists());
    sd.wait_for("the port", |s| std::net::TcpStream::connect(("127.0.0.1", s.port)).is_ok());
    assert!(!sd.dir.join("server.crt").exists(), "the mint must still be holding the certificate");

    sd.assert_handshake_fails("with the key alone").await;
    sd.assert_no_plain_http("with the key alone");
    sd.assert_alive("with the key alone");
    assert!(sd.output().contains("API TLS pair not loaded yet"), "no WARN:\n{}", sd.output());

    std::fs::write(sd.dir.join("go"), "").unwrap();
    sd.wait_for("the certificate", |s| s.dir.join("server.crt").exists());
    let start = Instant::now();
    loop {
        match sd.https("/healthz").await {
            Ok(r) if r.status() == 200 => break,
            // cp is not atomic: a handshake may land on half a certificate.
            _ if start.elapsed() < Duration::from_secs(10) => tokio::time::sleep(Duration::from_millis(50)).await,
            r => panic!("https never answered after the pair: {:?}\n{}", r, sd.output()),
        }
    }
    sd.assert_https_ok().await;
    sd.assert_no_plain_http("with the pair");

    sd.assert_alive("after the pair");
    assert_eq!(sd.child.id(), pid);
    let worker = sd.process("worker").await;
    assert_eq!(worker["state"], "running", "{}", worker);
    assert_eq!(worker["run"], 1, "the worker was restarted: {}", worker);
    assert_eq!(sd.process("mint").await["exit_code"], 0);
}

/// A stormd restarted with the two files disagreeing (stormcert was killed
/// between them) still starts, since its mint is what repairs them.
#[tokio::test]
async fn a_mismatched_pair_at_start_is_not_fatal() {
    let mut sd = Instance::start(
        "mismatch",
        |dir| {
            std::fs::write(dir.join("server.key"), SERVER2_KEY).unwrap();
            std::fs::write(dir.join("server.crt"), SERVER_CERT).unwrap();
        },
        &resident("worker"),
    );
    sd.wait_for("the worker", |s| s.dir.join("worker.up").exists());
    sd.wait_for("the port", |s| std::net::TcpStream::connect(("127.0.0.1", s.port)).is_ok());
    sd.assert_handshake_fails("on a mismatched pair").await;
    sd.assert_no_plain_http("on a mismatched pair");
    assert!(sd.output().contains("does not match"), "the WARN names the cause:\n{}", sd.output());

    std::fs::write(sd.dir.join("server.crt"), format!("{}\n", SERVER2_CERT)).unwrap();
    sd.assert_https_ok().await;
    sd.assert_alive("after the repair");
}
