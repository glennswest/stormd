//! `long` (the night window): waves of stormd's main workload — starting,
//! keeping and stopping processes — at this pod's capacity, measured across
//! waves for slowdown and for what is left behind.
//!
//! Each wave starts a fresh stormd with a population of processes (mostly
//! long-running, some crashing once, some one-shots with dependents), waits
//! for all of them to settle, and stops it with SIGTERM. Alongside, one stormd
//! lives for the whole night and has its processes restarted through the API
//! every wave — the leak test for an init that runs for months.
//!
//! Measured per wave: time to settle, time to stop, leftover processes, and
//! the long-lived stormd's memory and file descriptors. A wave slower than the
//! first of its size, a leftover process, or a residue that grows is a
//! failure even when every operation succeeded.
//!
//! Sizes come from this pod's own allowance (CPUs, cgroup memory and pid
//! limits), never from assumptions about the machine.

use std::time::{Duration, Instant};

use serde_json::Value;

use crate::env::Env;
use crate::harness::{Opts, Stormd};
use crate::report::{Outcome, Report};

/// Headroom kept back from the window for the last wave's drain and report.
const MARGIN: Duration = Duration::from_secs(60);

pub fn run(env: &Env, r: &mut Report) {
    let cap = capacity();
    r.record(
        "capacity",
        Outcome::Pass(format!(
            "{} CPUs, memory limit {} MiB, pid limit {} → up to {} processes per wave",
            cap.cpus,
            cap.mem_bytes / (1 << 20),
            cap.pids.map(|p| p.to_string()).unwrap_or_else(|| "none".into()),
            cap.procs
        )),
        0,
        None,
    );

    // The long-lived instance: half a wave, restarted through the API.
    let resident_n = (cap.procs / 2).max(4);
    let mut resident = match Stormd::start(env, "resident", &population(resident_n, false), Opts::default()) {
        Ok(s) => s,
        Err(e) => {
            r.record("resident-up", Outcome::Infra(e), 0, None);
            return;
        }
    };
    if !r.run("resident-up", || match settle(&mut resident, resident_n, Duration::from_secs(300)) {
        Ok(d) => Outcome::Pass(format!("{resident_n} processes settled in {} ms", d.as_millis())),
        Err(e) => Outcome::Fail(e),
    }) {
        return;
    }
    let resident_base = (resident.rss_kb().unwrap_or(0), resident.fds().unwrap_or(0));

    let sizes = [1.0, 0.5, 0.75];
    // First wave's settle time per size, the baseline each later one is held to.
    let mut first_settle: [Option<Duration>; 3] = [None; 3];
    let mut first_regressed: Option<u32> = None;
    let mut wave: u32 = 0;
    let mut last_wave = Duration::from_secs(0);

    loop {
        // Room for one more wave like the last, plus the drain?
        if wave > 0 && env.remaining() < last_wave * 2 + MARGIN {
            break;
        }
        wave += 1;
        let si = (wave as usize - 1) % sizes.len();
        let n = ((cap.procs as f64 * sizes[si]) as usize).max(4);
        let t = Instant::now();
        let m = one_wave(env, wave, n);
        last_wave = t.elapsed();

        // The resident: churn, then read its residue.
        let churned = churn(&mut resident, resident_n.min(64));
        let rss = resident.rss_kb().unwrap_or(0);
        let fds = resident.fds().unwrap_or(0);

        let mut problems = Vec::new();
        match &m {
            Ok(m) => {
                if m.leftover > 0 {
                    problems.push(format!("{} processes outlived stormd", m.leftover));
                }
                match first_settle[si] {
                    None => first_settle[si] = Some(m.settle),
                    Some(base) if m.settle > base * 2 + Duration::from_secs(1) => problems.push(format!(
                        "settled in {} ms, over twice the first wave of this size ({} ms)",
                        m.settle.as_millis(),
                        base.as_millis()
                    )),
                    _ => {}
                }
            }
            Err(e) => problems.push(e.clone()),
        }
        if let Err(e) = &churned {
            problems.push(format!("resident: {e}"));
        }
        // Growth beyond noise: half again the memory plus 8 MiB, or 16 fds.
        if rss > resident_base.0 * 3 / 2 + 8192 {
            problems.push(format!("resident stormd RSS {} KiB, from {} KiB", rss, resident_base.0));
        }
        if fds > resident_base.1 + 16 {
            problems.push(format!("resident stormd has {fds} fds, from {}", resident_base.1));
        }
        if resident.exit_status().is_some() {
            problems.push(format!("resident stormd exited: {}", resident.tail(4)));
        }

        let (settle, stop, leftover) = match &m {
            Ok(m) => (m.settle.as_millis(), m.stop.as_millis(), m.leftover),
            Err(_) => (0, 0, 0),
        };
        let extra = format!(
            "\"wave\": {wave}, \"procs\": {n}, \"settle_ms\": {settle}, \"stop_ms\": {stop}, \"leftover\": {leftover}, \
             \"resident_rss_kb\": {rss}, \"resident_fds\": {fds}, \"resident_restarted\": {}",
            churned.as_ref().copied().unwrap_or(0)
        );
        let outcome = if problems.is_empty() {
            Outcome::Pass(format!("{n} processes: settled {settle} ms, stopped {stop} ms"))
        } else {
            first_regressed.get_or_insert(wave);
            Outcome::Fail(problems.join("; "))
        };
        r.record(&format!("wave-{wave}"), outcome, last_wave.as_millis(), Some(&extra));
        if resident.exit_status().is_some() {
            break;
        }
    }

    let stopped = resident.terminate(Duration::from_secs(60));
    r.record(
        "trend",
        match (first_regressed, stopped) {
            (None, Ok((Some(0), _))) => Outcome::Pass(format!("{wave} waves, none regressed; resident stopped cleanly")),
            (Some(w), _) => Outcome::Fail(format!("first regressed at wave {w} of {wave}")),
            (None, Ok((c, _))) => Outcome::Fail(format!("resident exited {c:?} on SIGTERM")),
            (None, Err(e)) => Outcome::Fail(e),
        },
        0,
        Some(&format!("\"waves\": {wave}, \"first_regressed\": {}", first_regressed.map(|w| w.to_string()).unwrap_or_else(|| "null".into()))),
    );
}

struct Wave {
    settle: Duration,
    stop: Duration,
    leftover: usize,
}

fn one_wave(env: &Env, wave: u32, n: usize) -> Result<Wave, String> {
    let mut sd = Stormd::start(env, &format!("wave-{wave}"), &population(n, true), Opts::default())?;
    let settle = settle(&mut sd, n, Duration::from_secs(600))?;
    let (code, stop) = sd.terminate(Duration::from_secs(60))?;
    if code != Some(0) {
        return Err(format!("stormd exited {code:?} on SIGTERM: {}", sd.tail(4)));
    }
    let t = Instant::now();
    let mut left = sd.leftover().len();
    while left > 0 && t.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(100));
        left = sd.leftover().len();
    }
    // The wave's directory holds per-process logs; the next wave is a fresh
    // one, and a night of waves must not fill the results volume.
    let dir = sd.dir.clone();
    drop(sd);
    let _ = std::fs::remove_dir_all(dir);
    Ok(Wave { settle, stop, leftover: left })
}

/// `n` processes: one in ten crashes once, one in ten is a one-shot whose
/// neighbour depends on it; the rest stay up. With `one_shots` false (the
/// resident), everything stays up, so it can be restarted wholesale.
fn population(n: usize, one_shots: bool) -> String {
    let mut s = String::new();
    for i in 0..n {
        let (args, extra) = match i % 10 {
            3 if one_shots => (format!("\"touch-after\", \"50\", \"{{dir}}/done-{i}\""), "on_exit = \"stop\"".to_string()),
            4 if one_shots => ("\"sleep\"".to_string(), format!("depends_on = [\"p{}\"]", i - 1)),
            7 => (format!("\"crash-once\", \"{{dir}}/crashed-{i}\""), "restart_delay_secs = 1".to_string()),
            _ => ("\"sleep\"".to_string(), String::new()),
        };
        s.push_str(&format!(
            "\n[[process]]\nname = \"p{i}\"\ncommand = \"{{me}}\"\nargs = [\"helper\", {args}]\n{extra}\n"
        ));
    }
    s
}

/// Wait for the API, then for every process to settle: long-running ones
/// running, one-shots stopped after exiting 0. The time is from stormd's
/// start.
fn settle(sd: &mut Stormd, n: usize, t: Duration) -> Result<Duration, String> {
    let start = Instant::now();
    sd.wait_healthy(t)?;
    sd.wait_for(t.saturating_sub(start.elapsed()), &format!("{n} processes settled"), |s| {
        let v = s.json("/api/v1/processes")?;
        let all = v.as_array().ok_or("processes: not a list")?;
        let settled = all.iter().filter(|p| settled(p)).count();
        if all.len() == n && settled == n {
            Ok(Some(()))
        } else {
            Err(format!("{settled}/{n} settled"))
        }
    })?;
    Ok(start.elapsed())
}

fn settled(p: &Value) -> bool {
    p["state"] == "running" || (p["state"] == "stopped" && p["exit_code"] == 0)
}

/// Restart `k` of the resident's processes through the API, and wait for it
/// to settle again. Returns how many were restarted.
fn churn(sd: &mut Stormd, k: usize) -> Result<usize, String> {
    for i in 0..k {
        match sd.post(&format!("/api/v1/processes/p{i}/restart"), None)? {
            (200, _) => {}
            (s, b) => return Err(format!("restart p{i}: HTTP {s}: {b:.200}")),
        }
    }
    let n = sd.json("/api/v1/processes")?.as_array().map(|a| a.len()).unwrap_or(0);
    sd.wait_for(Duration::from_secs(120), "resident settled after churn", |s| {
        let v = s.json("/api/v1/processes")?;
        let settled = v.as_array().into_iter().flatten().filter(|p| settled(p)).count();
        Ok((settled == n).then_some(()))
    })?;
    Ok(k)
}

struct Capacity {
    cpus: usize,
    mem_bytes: u64,
    pids: Option<u64>,
    procs: usize,
}

/// This pod's allowance, and the wave size it supports: 16 processes per CPU,
/// at most one per 8 MiB of memory and a quarter of the pid limit (stormd and
/// the resident share it), between 8 and 1024.
fn capacity() -> Capacity {
    let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let cg = |f: &str| {
        std::fs::read_to_string(format!("/sys/fs/cgroup/{f}"))
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
    };
    let meminfo_avail = std::fs::read_to_string("/proc/meminfo").ok().and_then(|s| {
        s.lines()
            .find(|l| l.starts_with("MemAvailable:"))?
            .split_whitespace()
            .nth(1)?
            .parse::<u64>()
            .ok()
            .map(|kb| kb * 1024)
    });
    let mem_bytes = match (cg("memory.max"), meminfo_avail) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => 512 << 20,
    };
    let pids = cg("pids.max");
    let mut procs = cpus * 16;
    procs = procs.min((mem_bytes / (8 << 20)) as usize);
    if let Some(p) = pids {
        procs = procs.min((p / 4) as usize);
    }
    Capacity { cpus, mem_bytes, pids, procs: procs.clamp(8, 1024) }
}
