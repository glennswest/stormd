use crate::config::CronJobConfig;
use crate::events::{EventBus, EventKind};
use chrono::Utc;
use cron::Schedule;
use serde::Serialize;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use stormlog::types::{LogStream, Severity};
use stormlog::StormLog;
use tokio::process::Command;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

#[derive(Debug, Clone, Serialize)]
pub struct CronJobStatus {
    pub name: String,
    pub schedule: String,
    pub last_run: Option<String>,
    pub last_exit_code: Option<i32>,
    pub next_run: Option<String>,
    pub run_count: u64,
    pub fail_count: u64,
}

struct CronJobState {
    config: CronJobConfig,
    parsed_schedule: Schedule,
    /// When it fires next. Kept, not recomputed: see `run`.
    next_run: Option<chrono::DateTime<Utc>>,
    last_run: Option<chrono::DateTime<Utc>>,
    last_exit_code: Option<i32>,
    run_count: u64,
    fail_count: u64,
    /// A run of it is in progress (stormd#10).
    running: bool,
}

pub struct CronScheduler {
    jobs: RwLock<HashMap<String, CronJobState>>,
    stormlog: Arc<StormLog>,
    event_bus: Arc<EventBus>,
    shutdown: tokio::sync::watch::Sender<bool>,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
}

impl CronScheduler {
    pub fn new(stormlog: Arc<StormLog>, event_bus: Arc<EventBus>) -> Self {
        let (shutdown, shutdown_rx) = tokio::sync::watch::channel(false);
        Self {
            jobs: RwLock::new(HashMap::new()),
            stormlog,
            event_bus,
            shutdown,
            shutdown_rx,
        }
    }

    pub async fn register_jobs(&self, configs: &[CronJobConfig]) -> anyhow::Result<()> {
        let mut jobs = self.jobs.write().await;
        for cfg in configs {
            let schedule = Schedule::from_str(&cfg.schedule)
                .map_err(|e| anyhow::anyhow!("invalid cron expression '{}': {}", cfg.schedule, e))?;
            info!(name = %cfg.name, schedule = %cfg.schedule, "registered cron job");
            jobs.insert(
                cfg.name.clone(),
                CronJobState {
                    config: cfg.clone(),
                    next_run: schedule.upcoming(Utc).next(),
                    parsed_schedule: schedule,
                    last_run: None,
                    last_exit_code: None,
                    run_count: 0,
                    fail_count: 0,
                    running: false,
                },
            );
        }
        Ok(())
    }

    pub async fn run(self: Arc<Self>) {
        let mut rx = self.shutdown_rx.clone();
        loop {
            let now = Utc::now();
            let mut next_wake: Option<chrono::DateTime<Utc>> = None;
            let mut jobs_to_run = Vec::new();

            // **No job ever ran** (stormd#21). This asked the schedule for
            // `upcoming().next()` each time round and ran the job when that
            // was not in the future — but `upcoming` only ever yields times
            // after now, so the loop slept until a fire time, woke, and was
            // told the one after it, forever. The fire time is kept per job
            // and advanced when it has come.
            {
                let mut jobs = self.jobs.write().await;
                for (name, state) in jobs.iter_mut() {
                    let Some(next) = state.next_run else { continue };
                    if next <= now {
                        jobs_to_run.push(name.clone());
                        state.next_run = state.parsed_schedule.after(&now).next();
                    }
                    if let Some(next) = state.next_run {
                        match &next_wake {
                            Some(current) if next < *current => next_wake = Some(next),
                            None => next_wake = Some(next),
                            _ => {}
                        }
                    }
                }
            }

            // Each job on its own task (stormd#10): a long job used to hold up
            // every other job for up to its timeout, since they ran one after
            // another inside this loop. A job whose last run is still going
            // skips this fire time rather than piling up runs.
            for name in jobs_to_run {
                let busy = {
                    let mut jobs = self.jobs.write().await;
                    match jobs.get_mut(&name) {
                        Some(s) if s.running => true,
                        Some(s) => {
                            s.running = true;
                            false
                        }
                        None => continue,
                    }
                };
                if busy {
                    warn!(job = %name, "cron job still running from its last fire time — skipping this one");
                    continue;
                }
                let this = Arc::clone(&self);
                tokio::spawn(async move { this.execute_job(&name).await });
            }

            let sleep_dur = next_wake
                // Already past (a job ran long): go round again now.
                .map(|t| (t - Utc::now()).to_std().unwrap_or(std::time::Duration::ZERO))
                .unwrap_or(std::time::Duration::from_secs(1));

            tokio::select! {
                _ = tokio::time::sleep(sleep_dur) => {}
                _ = rx.changed() => {
                    if *rx.borrow() {
                        info!("cron scheduler shutting down");
                        return;
                    }
                }
            }
        }
    }

    async fn execute_job(&self, name: &str) {
        let (config, timeout) = {
            let jobs = self.jobs.read().await;
            let state = match jobs.get(name) {
                Some(s) => s,
                None => return,
            };
            (state.config.clone(), state.config.timeout_secs)
        };

        info!(job = %name, command = %config.command, "executing cron job");

        let mut cmd = Command::new(&config.command);
        // A timeout drops the child: kill it then, rather than leave it
        // running unsupervised (stormd#10).
        cmd.kill_on_drop(true);
        cmd.args(&config.args);
        // **Output was held whole in stormd** (stormd#95): both streams were
        // piped and `wait_with_output` kept every byte until the job exited
        // — `capture_output = false` included — so a noisy job could run PID
        // 1 out of memory. Off, the output goes nowhere; on, it is streamed
        // into the log line by line while the job runs.
        let out = || if config.capture_output { std::process::Stdio::piped() } else { std::process::Stdio::null() };
        cmd.stdout(out());
        cmd.stderr(out());
        for (k, v) in &config.env {
            cmd.env(k, v);
        }

        let result = match cmd.spawn() {
            Ok(mut child) => {
                let cron_name = format!("cron.{}", name);
                let mut readers = Vec::new();
                if let Some(o) = child.stdout.take() {
                    let (log, n) = (self.stormlog.clone(), cron_name.clone());
                    readers.push(tokio::spawn(async move { log.capture_stream(&n, o, LogStream::Stdout, Severity::Info).await }));
                }
                if let Some(e) = child.stderr.take() {
                    let (log, n) = (self.stormlog.clone(), cron_name.clone());
                    readers.push(tokio::spawn(async move { log.capture_stream(&n, e, LogStream::Stderr, Severity::Warning).await }));
                }
                let timeout_dur = tokio::time::Duration::from_secs(timeout);
                let code = match tokio::time::timeout(timeout_dur, child.wait()).await {
                    Ok(Ok(status)) => status.code(),
                    Ok(Err(e)) => {
                        error!(job = %name, error = %e, "cron job execution error");
                        None
                    }
                    Err(_) => {
                        // Killed and reaped before going on.
                        let _ = child.kill().await;
                        warn!(job = %name, timeout_secs = timeout, "cron job timed out — killed");
                        None
                    }
                };
                // The last lines still in the pipes; a pipe held open by
                // something the job left behind is let go 5 s after the job
                // ended — one deadline for both readers, not 5 s each.
                let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);
                for mut h in readers {
                    if tokio::time::timeout_at(deadline, &mut h).await.is_err() {
                        warn!(job = %name, "cron job output still open after it ended — capture stopped");
                        h.abort();
                        let _ = h.await;
                    }
                }
                code
            }
            Err(e) => {
                error!(job = %name, error = %e, "failed to spawn cron job");
                None
            }
        };

        let success = result == Some(0);
        {
            let mut jobs = self.jobs.write().await;
            if let Some(state) = jobs.get_mut(name) {
                state.last_run = Some(Utc::now());
                state.last_exit_code = result;
                state.run_count += 1;
                state.running = false;
                if !success {
                    state.fail_count += 1;
                }
            }
        }

        if success {
            self.event_bus
                .emit_simple(EventKind::CronExecuted, Some(name.to_string()))
                .await;
        } else {
            self.event_bus
                .emit_simple(EventKind::CronFailed, Some(name.to_string()))
                .await;
        }
    }

    pub async fn get_status(&self) -> Vec<CronJobStatus> {
        let jobs = self.jobs.read().await;
        let mut statuses = Vec::new();
        for (name, state) in jobs.iter() {
            let next_run = state.next_run.map(|t| t.to_rfc3339());
            statuses.push(CronJobStatus {
                name: name.clone(),
                schedule: state.config.schedule.clone(),
                last_run: state.last_run.map(|t| t.to_rfc3339()),
                last_exit_code: state.last_exit_code,
                next_run,
                run_count: state.run_count,
                fail_count: state.fail_count,
            });
        }
        statuses.sort_by(|a, b| a.name.cmp(&b.name));
        statuses
    }

    pub fn shutdown(&self) {
        let _ = self.shutdown.send(true);
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use cron::Schedule;
    use std::str::FromStr;

    /// The fire time the scheduler keeps: due once reached, then advanced
    /// past now — the step stormd#21 was missing.
    #[test]
    fn a_kept_fire_time_comes_due_and_advances() {
        let s = Schedule::from_str("* * * * * *").unwrap();
        let t0 = Utc.with_ymd_and_hms(2026, 9, 26, 12, 0, 0).unwrap();
        let next = s.after(&t0).next().unwrap();
        assert_eq!(next, t0 + chrono::Duration::seconds(1));
        // At that second the job is due; asking `upcoming` then, as the old
        // loop did, only ever says "later".
        let now = next;
        assert!(next <= now);
        let after = s.after(&now).next().unwrap();
        assert!(after > now);
        assert_eq!(after, now + chrono::Duration::seconds(1));
    }
}

#[cfg(test)]
mod run_tests {
    use super::CronScheduler;
    use std::sync::Arc;
    use std::time::Duration;

    fn scheduler(dir: &std::path::Path) -> Arc<CronScheduler> {
        let cfg: crate::config::Config = toml::from_str(&format!(
            "[general]\nname = \"t\"\nlog_dir = \"{}\"\n[stormlog.mcast]\ngroup = \"off\"\n",
            dir.display()
        ))
        .unwrap();
        let mut log_cfg = cfg.stormlog.clone();
        log_cfg.file.log_dir = dir.to_path_buf();
        let bus = Arc::new(crate::events::EventBus::new(cfg.events.clone(), "t".into()));
        let log = Arc::new(stormlog::StormLog::new(log_cfg, "t"));
        Arc::new(CronScheduler::new(log, bus))
    }

    /// stormd#10: a timed-out job is killed, not left running.
    #[tokio::test]
    async fn a_timeout_kills_the_job() {
        let dir = std::env::temp_dir().join(format!("stormd-cron-kill-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("survived");
        let sched = scheduler(&dir);
        let job: crate::config::CronJobConfig = toml::from_str(&format!(
            "name = \"slow\"\nschedule = \"0 0 0 1 1 *\"\ncommand = \"/bin/sh\"\n\
             args = [\"-c\", \"sleep 3; touch {}\"]\ntimeout_secs = 1\n",
            marker.display()
        ))
        .unwrap();
        sched.register_jobs(&[job]).await.unwrap();
        sched.execute_job("slow").await;
        tokio::time::sleep(Duration::from_secs(4)).await;
        let survived = marker.exists();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!survived, "the job ran on past its timeout");
    }

    /// Jobs run side by side: a long one does not hold up a short one.
    #[tokio::test]
    async fn jobs_do_not_wait_for_each_other() {
        let dir = std::env::temp_dir().join(format!("stormd-cron-par-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let quick = dir.join("quick");
        let sched = scheduler(&dir);
        let jobs: Vec<crate::config::CronJobConfig> = vec![
            toml::from_str("name = \"a-long\"\nschedule = \"* * * * * *\"\ncommand = \"/bin/sleep\"\nargs = [\"5\"]\ntimeout_secs = 10\n").unwrap(),
            toml::from_str(&format!(
                "name = \"b-quick\"\nschedule = \"* * * * * *\"\ncommand = \"/bin/sh\"\nargs = [\"-c\", \"echo x >> {}\"]\n",
                quick.display()
            ))
            .unwrap(),
        ];
        sched.register_jobs(&jobs).await.unwrap();
        let s = sched.clone();
        let h = tokio::spawn(async move { s.run().await });
        tokio::time::sleep(Duration::from_millis(3500)).await;
        h.abort();
        let runs = std::fs::read_to_string(&quick).map(|t| t.lines().count()).unwrap_or(0);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(runs >= 2, "the quick job ran {runs} times in 3.5 s beside a 5 s job");
    }
}

/// stormd#95 (b1dfa55675ce): cron output is streamed, never held whole, and
/// goes nowhere when not captured; a timeout still kills and reaps.
#[cfg(test)]
mod output_tests {
    use super::CronScheduler;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn setup(label: &str) -> (Arc<CronScheduler>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("stormd-cron-out-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cfg: crate::config::Config = toml::from_str(&format!(
            "[general]\nname = \"t\"\nlog_dir = \"{}\"\n[stormlog.mcast]\ngroup = \"off\"\n",
            dir.display()
        ))
        .unwrap();
        let mut log_cfg = cfg.stormlog.clone();
        log_cfg.file.log_dir = dir.clone();
        let bus = Arc::new(crate::events::EventBus::new(cfg.events.clone(), "t".into()));
        let log = Arc::new(stormlog::StormLog::new(log_cfg, "t"));
        (Arc::new(CronScheduler::new(log, bus)), dir)
    }

    async fn job(s: &CronScheduler, name: &str, script: &str, extra: &str) {
        let j: crate::config::CronJobConfig = toml::from_str(&format!(
            "name = \"{name}\"\nschedule = \"0 0 0 1 1 *\"\ncommand = \"/bin/sh\"\nargs = [\"-c\", '''{script}''']\n{extra}\n"
        ))
        .unwrap();
        s.register_jobs(&[j]).await.unwrap();
    }

    /// A line is in the log while the job is still running — not held
    /// until it exits.
    #[tokio::test]
    async fn output_is_logged_as_the_job_runs() {
        let (s, dir) = setup("stream");
        job(&s, "stream", "echo first; sleep 3; echo last >&2", "capture_output = true").await;
        let s2 = s.clone();
        let run = tokio::spawn(async move { s2.execute_job("stream").await });
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let during = s.stormlog.tail("cron.stream");
        run.await.unwrap();
        let after = s.stormlog.tail("cron.stream");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(during, vec!["first"], "the first line waited for the job to end");
        assert_eq!(after, vec!["first", "last"]);
    }

    /// `capture_output = false`: the job's stdout and stderr are /dev/null —
    /// nothing reaches stormd at all.
    #[tokio::test]
    async fn uncaptured_output_goes_nowhere() {
        let (s, dir) = setup("off");
        let fds = dir.join("fds");
        job(
            &s,
            "quiet",
            // The shell's own fds ($$), not readlink's — whose stdout is the
            // redirect.
            &format!("readlink /proc/$$/fd/1 > {f}; readlink /proc/$$/fd/2 >> {f}; echo noise", f = fds.display()),
            "capture_output = false",
        )
        .await;
        s.execute_job("quiet").await;
        let seen = std::fs::read_to_string(&fds).unwrap_or_default();
        let tail = s.stormlog.tail("cron.quiet");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(seen, "/dev/null\n/dev/null\n");
        assert!(tail.is_empty(), "{tail:?}");
    }

    /// A job writing far more than a line's cap without a newline is cut
    /// into bounded records as it goes, and finishes.
    #[tokio::test]
    async fn a_flood_without_newlines_is_cut_into_bounded_records() {
        let (s, dir) = setup("flood");
        job(&s, "flood", "head -c 4000000 /dev/zero | tr '\\0' x", "capture_output = true\ntimeout_secs = 60").await;
        s.execute_job("flood").await;
        let tail = s.stormlog.tail("cron.flood");
        let code = s.get_status().await.into_iter().find(|j| j.name == "flood").and_then(|j| j.last_exit_code);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(code, Some(0));
        assert!(!tail.is_empty());
        assert!(tail.iter().all(|l| l.len() <= stormlog::MAX_LINE), "a record past MAX_LINE");
    }

    /// A timeout kills and reaps the job, and a pipe something it left behind
    /// still holds does not hold the run up past the drain.
    #[tokio::test]
    async fn a_timeout_kills_and_reaps_even_with_the_pipe_held() {
        let (s, dir) = setup("timeout");
        let pid_file = dir.join("pid");
        job(
            &s,
            "stuck",
            &format!("echo $$ > {}; (sleep 30) & exec sleep 30", pid_file.display()),
            "capture_output = true\ntimeout_secs = 1",
        )
        .await;
        let t = Instant::now();
        s.execute_job("stuck").await;
        let took = t.elapsed();
        let pid: u32 = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
        let gone = !std::path::Path::new(&format!("/proc/{pid}")).exists();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(gone, "the job's process is still there (unkilled, or a zombie)");
        // 1 s timeout + one 5 s drain for both pipes, with slack.
        assert!(took < Duration::from_secs(8), "took {took:?}");
    }
}
