//! `medium` (< 30 min): stormd's features and failure paths, end to end. Each
//! test gets its own stormd, so one that fails leaves nothing for the next.

use std::time::Duration;

use crate::env::Env;
use crate::harness::{free_port, Opts, Stormd};
use crate::report::{Outcome, Report};

const S: fn(u64) -> Duration = Duration::from_secs;

pub fn run(env: &Env, r: &mut Report) {
    r.run("failed-one-shot-holds-dependents", || failed_one_shot(env, false));
    r.run("sigterm-with-parked-start-order", || failed_one_shot(env, true));
    r.run("no-restart-code-hold", || no_restart(env, "hold"));
    r.run("no-restart-code-fail", || no_restart(env, "fail"));
    r.run("on-failure-fail", || on_failure_fail(env));
    r.run("max-restarts-fails-container", || max_restarts(env));
    r.run("on-exit-restart", || on_exit_restart(env));
    r.run("liveness-restarts", || liveness(env));
    r.run("api-stop-start-restart", || api_control(env));
    r.run("api-shutdown-exit-code", || api_shutdown(env));
    r.run("auth-token", || auth(env));
    r.run("metrics", || metrics(env));
    r.run("components-feed", || components(env));
    r.run("cron-job-runs", || cron(env));
    r.run("bad-config-refused", || bad_config(env));
    r.run("node-stormd", || crate::short::node_stormd(env));
}

/// Start an instance, wait for its API, and hand it to `f`.
fn with(env: &Env, label: &str, body: &str, opts: Opts, f: impl FnOnce(&mut Stormd) -> Outcome) -> Outcome {
    let mut sd = match Stormd::start(env, label, body, opts) {
        Ok(s) => s,
        Err(e) => return Outcome::Infra(e),
    };
    if let Err(e) = sd.wait_healthy(S(15)) {
        return Outcome::Fail(e);
    }
    f(&mut sd)
}

fn proc(name: &str, args: &str, extra: &str) -> String {
    format!("\n[[process]]\nname = \"{name}\"\ncommand = \"{{me}}\"\nargs = [\"helper\", {args}]\n{extra}\n")
}

/// A dependent of a one-shot that failed under `on_failure = "ignore"` is
/// held (stormd#16) — and SIGTERM still stops stormd while it is (stormd#17).
fn failed_one_shot(env: &Env, then_sigterm: bool) -> Outcome {
    let body = proc("bad", "\"exit\", \"1\"", "on_exit = \"stop\"\non_failure = \"ignore\"")
        + &proc("held", "\"sleep\"", "depends_on = [\"bad\"]");
    let label = if then_sigterm { "parked" } else { "held" };
    with(env, label, &body, Opts::default(), |sd| {
        if let Err(e) = sd.wait_state("bad", "stopped", S(10)) {
            return Outcome::Fail(e);
        }
        std::thread::sleep(S(2));
        match sd.process("held") {
            Ok(p) if p["state"] != "pending" => {
                return Outcome::Fail(format!("the dependent of a failed one-shot started: {}", p["state"]))
            }
            Err(e) => return Outcome::Fail(e),
            _ => {}
        }
        if !then_sigterm {
            return if sd.output().contains("did not finish cleanly") {
                Outcome::Pass("held pending 2 s after bad exited 1; the WARN names it".into())
            } else {
                Outcome::Fail("held, but no WARN saying why".into())
            };
        }
        match sd.terminate(S(10)) {
            Ok((Some(0), d)) => Outcome::Pass(format!("exit 0 in {} ms with the start order parked", d.as_millis())),
            Ok((c, _)) => Outcome::Fail(format!("exited {c:?}, not 0")),
            Err(e) => Outcome::Fail(e),
        }
    })
}

/// `no_restart_exit_codes`: not restarted; `hold` keeps stormd up, `fail`
/// fails the container (stormd#2).
fn no_restart(env: &Env, action: &str) -> Outcome {
    let body = proc(
        "cfg",
        "\"exit\", \"78\", \"200\"",
        &format!("no_restart_exit_codes = [78]\non_no_restart = \"{action}\""),
    ) + &proc("bystander", "\"sleep\"", "");
    with(env, &format!("norestart-{action}"), &body, Opts::default(), |sd| {
        if action == "fail" {
            return match sd.wait_exit(S(15)) {
                Some((s, _)) if s.code() == Some(1) => Outcome::Pass("exit 78 → container failed, stormd exit 1".into()),
                Some((s, _)) => Outcome::Fail(format!("stormd exited {s}, want 1")),
                None => Outcome::Fail("stormd still up 15 s after a no-restart exit under 'fail'".into()),
            };
        }
        let p = match sd.wait_state("cfg", "failed", S(10)) {
            Ok(p) => p,
            Err(e) => return Outcome::Fail(e),
        };
        std::thread::sleep(S(2));
        let again = sd.process("cfg").unwrap_or_default();
        if again["restarts"] != 0 || again["state"] != "failed" {
            return Outcome::Fail(format!("restarted anyway: {again}"));
        }
        match sd.process("bystander") {
            Ok(b) if b["state"] == "running" => {
                Outcome::Pass(format!("failed with exit {}, not restarted, container still up", p["exit_code"]))
            }
            Ok(b) => Outcome::Fail(format!("the container did not stay up: bystander {}", b["state"])),
            Err(e) => Outcome::Fail(e),
        }
    })
}

fn on_failure_fail(env: &Env) -> Outcome {
    let body = proc("fatal", "\"exit\", \"3\", \"200\"", "on_failure = \"fail\"");
    with(env, "onfail", &body, Opts::default(), |sd| match sd.wait_exit(S(15)) {
        Some((s, _)) if s.code() == Some(1) => Outcome::Pass("exit 3 under on_failure=fail → stormd exit 1".into()),
        Some((s, _)) => Outcome::Fail(format!("stormd exited {s}, want 1")),
        None => Outcome::Fail("stormd still up 15 s after a fatal exit".into()),
    })
}

fn max_restarts(env: &Env) -> Outcome {
    let body = proc(
        "flaky",
        "\"exit\", \"1\", \"100\"",
        "max_restarts = 2\nrestart_delay_secs = 1\nrestart_window_secs = 300",
    );
    with(env, "maxrestarts", &body, Opts::default(), |sd| match sd.wait_exit(S(40)) {
        Some((s, d)) if s.code() == Some(1) => Outcome::Pass(format!(
            "2 restarts then container failed, stormd exit 1 after {} s",
            d.as_secs()
        )),
        Some((s, _)) => Outcome::Fail(format!("stormd exited {s}, want 1")),
        None => Outcome::Fail(format!("stormd still up 40 s later: {}", sd.tail(4))),
    })
}

fn on_exit_restart(env: &Env) -> Outcome {
    let body = proc("loop", "\"exit\", \"0\", \"300\"", "on_exit = \"restart\"\nrestart_delay_secs = 1");
    with(env, "onexit", &body, Opts::default(), |sd| {
        match sd.wait_for(S(20), "loop restarted twice", |s| {
            let p = s.process("loop")?;
            Ok((p["restarts"].as_u64() >= Some(2)).then_some(p))
        }) {
            Ok(p) if p["crashes"] == 0 => Outcome::Pass(format!("clean exits restarted ({} restarts, no crashes)", p["restarts"])),
            Ok(p) => Outcome::Fail(format!("clean exits counted as crashes: {p}")),
            Err(e) => Outcome::Fail(e),
        }
    })
}

/// A liveness probe that never passes gets the process killed and restarted.
fn liveness(env: &Env) -> Outcome {
    let dead = match free_port() {
        Ok(p) => p,
        Err(e) => return Outcome::Infra(e),
    };
    let body = proc(
        "stuck",
        "\"sleep\"",
        &format!(
            "restart_delay_secs = 1\nliveness = {{ type = \"tcp\", port = {dead}, interval_secs = 1, failure_threshold = 2, initial_delay_secs = 0, timeout_secs = 1 }}"
        ),
    );
    with(env, "liveness", &body, Opts::default(), |sd| {
        match sd.wait_for(S(30), "stuck restarted by liveness", |s| {
            let p = s.process("stuck")?;
            Ok((p["restarts"].as_u64() >= Some(1)).then_some(p))
        }) {
            Ok(p) => Outcome::Pass(format!("restarted ({} restarts) after the probe failed twice", p["restarts"])),
            Err(e) => Outcome::Fail(e),
        }
    })
}

fn api_control(env: &Env) -> Outcome {
    let body = proc("worker", "\"sleep\"", "");
    with(env, "control", &body, Opts::default(), |sd| {
        let step = |sd: &mut Stormd, action: &str, want: &str| -> Result<serde_json::Value, String> {
            match sd.post(&format!("/api/v1/processes/worker/{action}"), None)? {
                (200, _) => sd.wait_state("worker", want, S(10)),
                (s, b) => Err(format!("{action}: HTTP {s}: {b:.200}")),
            }
        };
        let mut run = || -> Result<String, String> {
            let p0 = sd.wait_state("worker", "running", S(10))?;
            step(sd, "stop", "stopped")?;
            let p1 = step(sd, "start", "running")?;
            // restart returns once the new process is spawned
            match sd.post("/api/v1/processes/worker/restart", None)? {
                (200, _) => {}
                (s, b) => return Err(format!("restart: HTTP {s}: {b:.200}")),
            }
            let p2 = sd.wait_for(S(10), "worker running under a new pid", |s| {
                let p = s.process("worker")?;
                Ok((p["state"] == "running" && p["pid"] != p1["pid"]).then_some(p))
            })?;
            match sd.post("/api/v1/processes/nosuch/stop", None)? {
                (s, _) if (400..500).contains(&s) || s == 500 => {}
                (s, _) => return Err(format!("stop of an unknown process: HTTP {s}")),
            }
            if p0["pid"] == p1["pid"] {
                return Err("start after stop kept the old pid".into());
            }
            Ok(format!("pids {} → stop → {} → restart → {}", p0["pid"], p1["pid"], p2["pid"]))
        };
        match run() {
            Ok(d) => Outcome::Pass(d),
            Err(e) => Outcome::Fail(e),
        }
    })
}

fn api_shutdown(env: &Env) -> Outcome {
    let body = proc("worker", "\"sleep\"", "");
    with(env, "shutdown", &body, Opts::default(), |sd| {
        if let Err(e) = sd.wait_state("worker", "running", S(10)) {
            return Outcome::Fail(e);
        }
        match sd.post("/api/v1/shutdown", Some("{\"exitCode\": 7}")) {
            Ok((200, _)) => {}
            Ok((s, b)) => return Outcome::Fail(format!("HTTP {s}: {b:.200}")),
            Err(e) => return Outcome::Fail(e),
        }
        match sd.wait_exit(S(15)) {
            Some((s, d)) if s.code() == Some(7) => {
                let left = sd.leftover();
                if left.is_empty() {
                    Outcome::Pass(format!("exit 7 in {} ms, worker stopped", d.as_millis()))
                } else {
                    Outcome::Fail(format!("exit 7 but {left:?} still running"))
                }
            }
            Some((s, _)) => Outcome::Fail(format!("exited {s}, want 7")),
            None => Outcome::Fail("still up 15 s after POST /api/v1/shutdown".into()),
        }
    })
}

fn auth(env: &Env) -> Outcome {
    let token = format!("t-{}", env.run_id);
    let body = proc("worker", "\"sleep\"", "");
    with(env, "auth", &body, Opts { token: Some(token.clone()) }, |sd| {
        let res = (|| -> Result<(), String> {
            match sd.get_as("/api/v1/processes", None)? {
                (401, _) => {}
                (s, _) => return Err(format!("no token: HTTP {s}, want 401")),
            }
            match sd.get_as("/api/v1/processes", Some("wrong"))? {
                (401, _) => {}
                (s, _) => return Err(format!("wrong token: HTTP {s}, want 401")),
            }
            match sd.get_as("/api/v1/processes", Some(&token))? {
                (200, _) => {}
                (s, _) => return Err(format!("right token: HTTP {s}, want 200")),
            }
            match sd.get_as("/api/v1/health", None)? {
                (200, _) => Ok(()),
                (s, _) => Err(format!("health without a token: HTTP {s}, want 200 (it is public)")),
            }
        })();
        match res {
            Ok(()) => Outcome::Pass("401 without or with a wrong token, 200 with it; health open".into()),
            Err(e) => Outcome::Fail(e),
        }
    })
}

fn metrics(env: &Env) -> Outcome {
    let body = proc("worker", "\"sleep\"", "");
    with(env, "metrics", &body, Opts::default(), |sd| {
        if let Err(e) = sd.wait_state("worker", "running", S(10)) {
            return Outcome::Fail(e);
        }
        match sd.get("/metrics") {
            Ok((200, b)) => {
                let want = ["stormd_up", "stormd_uptime_seconds", "stormd_process_state", "process_resident_memory_bytes"];
                let missing: Vec<_> = want.iter().filter(|m| !b.contains(*m)).collect();
                if !missing.is_empty() {
                    Outcome::Fail(format!("missing {missing:?}"))
                } else if !b.lines().any(|l| l.starts_with("stormd_process_state") && l.contains("worker")) {
                    Outcome::Fail("no stormd_process_state series for worker".into())
                } else {
                    Outcome::Pass(format!("{} series lines, worker's state among them", b.lines().filter(|l| !l.starts_with('#')).count()))
                }
            }
            Ok((s, _)) => Outcome::Fail(format!("HTTP {s}")),
            Err(e) => Outcome::Fail(e),
        }
    })
}

fn components(env: &Env) -> Outcome {
    let body = proc("worker", "\"sleep\"", "");
    with(env, "components", &body, Opts::default(), |sd| {
        if let Err(e) = sd.wait_state("worker", "running", S(10)) {
            return Outcome::Fail(e);
        }
        match sd.json("/api/v1/components") {
            Ok(v) => {
                let n = v.as_array().map(|a| a.len()).unwrap_or(0);
                if n > 0 && v.to_string().contains("worker") {
                    Outcome::Pass(format!("{n} component summaries, worker's among them"))
                } else {
                    Outcome::Fail(format!("{n} summaries, worker not among them"))
                }
            }
            Err(e) => Outcome::Fail(e),
        }
    })
}

fn cron(env: &Env) -> Outcome {
    let body = "\n[[cron]]\nname = \"tick\"\nschedule = \"* * * * * *\"\ncommand = \"{me}\"\n\
                args = [\"helper\", \"touch-after\", \"0\", \"{dir}/tick\"]\n"
        .to_string();
    with(env, "cron", &body, Opts::default(), |sd| {
        let file = sd.dir.join("tick");
        let t = std::time::Instant::now();
        while !file.exists() && t.elapsed() < S(10) {
            std::thread::sleep(Duration::from_millis(100));
        }
        if !file.exists() {
            return Outcome::Fail(format!("an every-second job did not run in 10 s: {}", sd.tail(4)));
        }
        match sd.json("/api/v1/cron") {
            Ok(v) if v.to_string().contains("tick") => Outcome::Pass(format!("ran within {} ms; listed by /api/v1/cron", t.elapsed().as_millis())),
            Ok(v) => Outcome::Fail(format!("ran, but /api/v1/cron does not list it: {v:.200}")),
            Err(e) => Outcome::Fail(e),
        }
    })
}

fn bad_config(env: &Env) -> Outcome {
    let mut sd = match Stormd::start_raw(env, "badconfig", "this is = = not toml [\n") {
        Ok(s) => s,
        Err(e) => return Outcome::Infra(e),
    };
    match sd.wait_exit(S(10)) {
        Some((s, _)) if s.code() == Some(1) => Outcome::Pass("refused with exit 1".into()),
        Some((s, _)) => Outcome::Fail(format!("exited {s}, want 1")),
        None => Outcome::Fail("still running on a config that does not parse".into()),
    }
}
