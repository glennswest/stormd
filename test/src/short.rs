//! `short` (< 2 min): stormd is up and doing its main job — one instance
//! starts a small container's worth of processes in order, honours a ready
//! probe and a one-shot, restarts a crash, serves its logs, and stops on
//! SIGTERM leaving nothing behind. Then the node's own stormds, if any answer.

use std::time::{Duration, Instant};

use crate::env::Env;
use crate::harness::{free_port, http_to, Opts, Stormd};
use crate::report::{Outcome, Report};

/// The API ports of the control plane's stormds (fastetcd 9081, rustkube
/// 9082–9085; README "How it ships"). A worker without a control plane has
/// none of these, which is a skip, not a failure.
const NODE_PORTS: [u16; 5] = [9081, 9082, 9083, 9084, 9085];

pub fn run(env: &Env, r: &mut Report) {
    let svc_port = match free_port() {
        Ok(p) => p,
        Err(e) => {
            r.record("api-up", Outcome::Infra(e), 0, None);
            return;
        }
    };
    let marker = format!("marker-{}", env.run_id);
    // Config order is start order. `probed` is gated on `svc`'s tcp ready
    // probe, and `svc` listens only after 300 ms, so `probed` connecting
    // proves the gate; `after` needs the file `once` writes 1.5 s in.
    let body = format!(
        r#"
[[process]]
name = "svc"
command = "{{me}}"
args = ["helper", "serve", "{svc_port}", "300"]
ready_probe = {{ type = "tcp", port = {svc_port}, interval_secs = 1 }}

[[process]]
name = "probed"
command = "{{me}}"
args = ["helper", "connect", "{svc_port}"]
on_exit = "stop"
on_failure = "ignore"
depends_on = ["svc"]

[[process]]
name = "once"
command = "{{me}}"
args = ["helper", "touch-after", "1500", "{{dir}}/key"]
on_exit = "stop"

[[process]]
name = "after"
command = "{{me}}"
args = ["helper", "require", "{{dir}}/key"]
on_exit = "stop"
on_failure = "ignore"
depends_on = ["once"]

[[process]]
name = "crashy"
command = "{{me}}"
args = ["helper", "crash-once", "{{dir}}/crashed"]
restart_delay_secs = 1

[[process]]
name = "talker"
command = "{{me}}"
args = ["helper", "say", "{marker}"]
"#
    );

    let mut sd = match Stormd::start(env, "short", &body, Opts::default()) {
        Ok(s) => s,
        Err(e) => {
            r.record("api-up", Outcome::Infra(e), 0, None);
            return;
        }
    };

    let up = r.run("api-up", || match sd.wait_healthy(Duration::from_secs(15)) {
        Ok(d) => Outcome::Pass(format!("/api/v1/health 200 after {} ms (stormd pid {})", d.as_millis(), sd.pid())),
        Err(e) => Outcome::Fail(e),
    });
    if !up {
        return;
    }

    r.run("ready-probe-gates", || {
        match sd.wait_state("probed", "stopped", Duration::from_secs(20)) {
            Ok(p) if p["exit_code"] == 0 => {
                Outcome::Pass("the dependent started after svc's tcp probe passed and reached it".into())
            }
            Ok(p) => Outcome::Fail(format!(
                "the dependent ran before svc was listening (exit {}): depends_on did not wait for the probe",
                p["exit_code"]
            )),
            Err(e) => Outcome::Fail(e),
        }
    });

    r.run("one-shot-then-dependent", || {
        match sd.wait_state("after", "stopped", Duration::from_secs(20)) {
            Ok(p) if p["exit_code"] == 0 => Outcome::Pass("the dependent ran after the one-shot finished its work".into()),
            Ok(p) => Outcome::Fail(format!(
                "the dependent ran before the one-shot's file existed (exit {}) — stormd#16",
                p["exit_code"]
            )),
            Err(e) => Outcome::Fail(e),
        }
    });

    r.run("restart-on-crash", || {
        match sd.wait_for(Duration::from_secs(20), "crashy restarted and running", |s| {
            let p = s.process("crashy")?;
            let ok = p["state"] == "running" && p["restarts"].as_u64() >= Some(1) && p["crashes"].as_u64() >= Some(1);
            Ok(ok.then_some(p))
        }) {
            Ok(p) => Outcome::Pass(format!("crashes {}, restarts {}, running", p["crashes"], p["restarts"])),
            Err(e) => Outcome::Fail(e),
        }
    });

    r.run("logs-api", || {
        let path = format!("/api/v1/logs/talker?search={marker}");
        match sd.wait_for(Duration::from_secs(10), "the marker in talker's log", |s| {
            let v = s.json(&path)?;
            let lines: Vec<String> =
                v["lines"].as_array().into_iter().flatten().filter_map(|l| l.as_str().map(String::from)).collect();
            let both = lines.iter().any(|l| l.contains("stdout")) && lines.iter().any(|l| l.contains("stderr"));
            Ok(both.then_some(lines.len()))
        }) {
            Ok(n) => Outcome::Pass(format!("{n} lines with the marker, stdout and stderr both captured")),
            Err(e) => Outcome::Fail(e),
        }
    });

    r.run("sigterm-shutdown", || {
        let running = sd.leftover().len();
        match sd.terminate(Duration::from_secs(15)) {
            Ok((code, d)) => {
                // The kills have landed by the time stormd exits; give the
                // kernel a moment to finish tearing the processes down.
                let t = Instant::now();
                let mut left = sd.leftover();
                while !left.is_empty() && t.elapsed() < Duration::from_secs(2) {
                    std::thread::sleep(Duration::from_millis(100));
                    left = sd.leftover();
                }
                if code != Some(0) {
                    Outcome::Fail(format!("exited {code:?} on SIGTERM, not 0: {}", sd.tail(4)))
                } else if !left.is_empty() {
                    Outcome::Fail(format!("stormd exited but its processes {left:?} are still running"))
                } else {
                    Outcome::Pass(format!("exit 0 in {} ms; {running} running processes stopped, none left", d.as_millis()))
                }
            }
            Err(e) => Outcome::Fail(e),
        }
    });

    r.run("node-stormd", || node_stormd(env));
}

/// The node's own stormds, read-only: `/api/v1/health` is open even when a
/// stormd has authentication on.
pub fn node_stormd(env: &Env) -> Outcome {
    if env.node.is_empty() {
        return Outcome::Skip("STORM_NODE not set".into());
    }
    let mut up = Vec::new();
    let mut other = Vec::new();
    for port in NODE_PORTS {
        let addr = format!("{}:{port}", env.node);
        match http_to(&addr, "GET", "/api/v1/health", None, None, Duration::from_secs(2)) {
            Ok((200, b)) if b.contains("ok") => up.push(port),
            Ok((s, _)) => other.push(format!("{port}: HTTP {s}")),
            Err(_) => {}
        }
    }
    // Something else may hold one of these ports on a machine we know
    // nothing about, so only a healthy answer counts either way.
    if up.is_empty() {
        return Outcome::Skip(format!(
            "no stormd healthy on {}:{NODE_PORTS:?} (no control plane here?); other answers: {other:?}",
            env.node
        ));
    }
    Outcome::Pass(format!("healthy on {}:{up:?}; other answers: {other:?}", env.node))
}
