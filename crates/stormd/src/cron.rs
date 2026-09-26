use crate::config::CronJobConfig;
use crate::events::{EventBus, EventKind};
use chrono::Utc;
use cron::Schedule;
use serde::Serialize;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use stormlog::types::{LogEntry, LogStream, Severity};
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

            for name in jobs_to_run {
                self.execute_job(&name).await;
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
        cmd.args(&config.args);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        for (k, v) in &config.env {
            cmd.env(k, v);
        }

        let result = match cmd.spawn() {
            Ok(child) => {
                let timeout_dur = tokio::time::Duration::from_secs(timeout);
                match tokio::time::timeout(timeout_dur, child.wait_with_output()).await {
                    Ok(Ok(output)) => {
                        if config.capture_output {
                            let cron_name = format!("cron.{}", name);
                            if let Ok(s) = String::from_utf8(output.stdout) {
                                for line in s.lines() {
                                    let entry = LogEntry::new(&cron_name, LogStream::Stdout, line);
                                    self.stormlog.write_entry(entry).await;
                                }
                            }
                            if let Ok(s) = String::from_utf8(output.stderr) {
                                for line in s.lines() {
                                    let entry = LogEntry::new(&cron_name, LogStream::Stderr, line)
                                        .with_severity(Severity::Warning);
                                    self.stormlog.write_entry(entry).await;
                                }
                            }
                        }
                        output.status.code()
                    }
                    Ok(Err(e)) => {
                        error!(job = %name, error = %e, "cron job execution error");
                        None
                    }
                    Err(_) => {
                        warn!(job = %name, timeout_secs = timeout, "cron job timed out");
                        None
                    }
                }
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
