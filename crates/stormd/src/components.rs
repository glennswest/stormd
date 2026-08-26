//! The component summary contract — one uniform shape that every part of the
//! system reports itself in, and the only thing the dashboards know how to
//! render. The web UI and stormsh both draw from this feed, so a subsystem
//! that implements a summary here appears in both, and neither UI can drift
//! from the other because neither owns the model.

use crate::api::AppState;
use crate::supervisor::ProcessState;
use serde::Serialize;

/// Component health, in the order a viewer sorts by: broken first.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Health {
    Error,
    Warn,
    Ok,
    Idle,
    Unknown,
}

/// One headline number on a component's card. `tone` is a rendering hint
/// ("ok" | "warn" | "error" | "muted" | "accent"), not a semantic — health
/// lives on the component.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Metric {
    pub label: String,
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tone: Option<String>,
}

impl Metric {
    fn new(label: &str, value: impl Into<String>) -> Self {
        Self {
            label: label.to_string(),
            value: value.into(),
            unit: None,
            tone: None,
        }
    }

    fn unit(mut self, unit: &str) -> Self {
        self.unit = Some(unit.to_string());
        self
    }

    fn tone(mut self, tone: &str) -> Self {
        self.tone = Some(tone.to_string());
        self
    }
}

/// An operation a viewer may invoke on a component. The path is a real API
/// path, so a renderer needs no per-kind knowledge to wire a button.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Action {
    pub id: String,
    pub label: String,
    pub method: String,
    pub path: String,
    pub enabled: bool,
    pub danger: bool,
}

impl Action {
    fn process(id: &str, label: &str, process: &str, enabled: bool, danger: bool) -> Self {
        Self {
            id: id.to_string(),
            label: label.to_string(),
            method: "POST".to_string(),
            path: format!("/api/v1/processes/{}/{}", process, id),
            enabled,
            danger,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ComponentSummary {
    /// Stable identity, e.g. "system", "process:web", "cron:backup".
    pub id: String,
    /// "system" | "process" | "plugin" | "cron" | "storage" | "logs" | "updater"
    pub kind: String,
    pub label: String,
    pub health: Health,
    /// One human line: what a viewer would say this component is doing.
    pub detail: String,
    pub metrics: Vec<Metric>,
    pub actions: Vec<Action>,
    /// UI route within the SPA (hash route); a TUI ignores it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

/// Assemble the summary of every component in the system, in display order:
/// the system itself, then what it supervises, then what it keeps.
pub async fn collect(state: &AppState) -> Vec<ComponentSummary> {
    let mut out = Vec::new();

    let statuses = state.supervisor.get_all_statuses().await;
    let sys = state.stats.get_stats().await;

    // --- The supervisor itself ---
    let running = statuses
        .iter()
        .filter(|s| s.state == ProcessState::Running)
        .count();
    let failed = statuses
        .iter()
        .filter(|s| s.state == ProcessState::Failed)
        .count();
    let restarts: u32 = statuses.iter().map(|s| s.restarts).sum();

    let mut metrics = vec![
        Metric::new("uptime", format_duration(sys.uptime_secs)),
        Metric::new("processes", format!("{}/{}", running, statuses.len()))
            .tone(if failed > 0 { "warn" } else { "ok" }),
        Metric::new("restarts", restarts.to_string()),
    ];
    if let Some(mem) = &sys.memory {
        metrics.push(Metric::new("rss", format_bytes(mem.rss_bytes)));
    }
    out.push(ComponentSummary {
        id: "system".to_string(),
        kind: "system".to_string(),
        label: state.container_name.clone(),
        health: if failed > 0 { Health::Warn } else { Health::Ok },
        detail: format!(
            "up {} · {}/{} running",
            format_duration(sys.uptime_secs),
            running,
            statuses.len()
        ),
        metrics,
        actions: Vec::new(),
        link: None,
    });

    // --- Supervised processes ---
    // A process with a plugin UI is presented as that plugin: same lifecycle,
    // but labeled and linked as the thing the viewer knows it as.
    for p in &statuses {
        let plugin = state.ui_plugins.iter().find(|u| u.name == p.name);

        let health = match p.state {
            ProcessState::Running => {
                if p.liveness_failures > 0 {
                    Health::Warn
                } else {
                    Health::Ok
                }
            }
            ProcessState::Failed => Health::Error,
            ProcessState::Starting | ProcessState::Restarting => Health::Warn,
            ProcessState::Stopping | ProcessState::Stopped | ProcessState::Pending => Health::Idle,
        };

        let mut detail = format!("{:?}", p.state).to_lowercase();
        if let Some(pid) = p.pid {
            detail.push_str(&format!(" · pid {}", pid));
        }
        if p.state == ProcessState::Running {
            if let Some(u) = p.uptime_secs {
                detail.push_str(&format!(" · up {}", format_duration(u)));
            }
        } else if let Some(code) = p.exit_code {
            detail.push_str(&format!(" · exit {}", code));
        }

        let mut metrics = vec![
            Metric::new("restarts", p.restarts.to_string())
                .tone(if p.restarts > 0 { "warn" } else { "muted" }),
            Metric::new("crashes", p.crashes.to_string())
                .tone(if p.crashes > 0 { "error" } else { "muted" }),
        ];
        if p.has_liveness {
            metrics.push(
                Metric::new("liveness fails", p.liveness_failures.to_string())
                    .tone(if p.liveness_failures > 0 { "warn" } else { "ok" }),
            );
        }

        let stopped = matches!(
            p.state,
            ProcessState::Stopped | ProcessState::Failed | ProcessState::Pending
        );
        let actions = vec![
            Action::process("start", "Start", &p.name, stopped, false),
            Action::process("stop", "Stop", &p.name, !stopped, true),
            Action::process("restart", "Restart", &p.name, !stopped, false),
        ];

        out.push(ComponentSummary {
            id: format!("process:{}", p.name),
            kind: if plugin.is_some() { "plugin" } else { "process" }.to_string(),
            label: plugin.map(|u| u.label.clone()).unwrap_or_else(|| p.name.clone()),
            health,
            detail,
            metrics,
            actions,
            link: Some(match plugin {
                Some(u) => format!("#/ext/{}", u.name),
                None => format!("#/process/{}", p.name),
            }),
        });
    }

    // --- Cron jobs ---
    for job in state.cron_scheduler.get_status().await {
        let health = if job.fail_count > 0 {
            Health::Warn
        } else if job.run_count == 0 {
            Health::Idle
        } else {
            Health::Ok
        };
        let detail = match &job.next_run {
            Some(next) => format!("next {}", next),
            None => "not scheduled".to_string(),
        };
        out.push(ComponentSummary {
            id: format!("cron:{}", job.name),
            kind: "cron".to_string(),
            label: job.name.clone(),
            health,
            detail,
            metrics: vec![
                Metric::new("schedule", job.schedule.clone()).tone("muted"),
                Metric::new("runs", job.run_count.to_string()),
                Metric::new("failed", job.fail_count.to_string())
                    .tone(if job.fail_count > 0 { "error" } else { "muted" }),
            ],
            actions: Vec::new(),
            link: None,
        });
    }

    // --- Storage ---
    for m in crate::stats::StatsCollector::get_mounts() {
        let health = if m.use_percent >= 95.0 {
            Health::Error
        } else if m.use_percent >= 85.0 {
            Health::Warn
        } else {
            Health::Ok
        };
        out.push(ComponentSummary {
            id: format!("mount:{}", m.mount_point),
            kind: "storage".to_string(),
            label: m.mount_point.clone(),
            health,
            detail: format!(
                "{} of {} used · {}",
                format_bytes(m.used_bytes),
                format_bytes(m.total_bytes),
                m.fs_type
            ),
            metrics: vec![
                Metric::new("used", format!("{:.0}", m.use_percent))
                    .unit("%")
                    .tone(match health {
                        Health::Error => "error",
                        Health::Warn => "warn",
                        _ => "ok",
                    }),
                Metric::new("free", format_bytes(m.avail_bytes)),
            ],
            actions: Vec::new(),
            link: None,
        });
    }

    // --- Logs ---
    let (files, bytes) = log_dir_totals(&state.log_dir).await;
    out.push(ComponentSummary {
        id: "logs".to_string(),
        kind: "logs".to_string(),
        label: "Logs".to_string(),
        health: Health::Ok,
        detail: format!("{} files · {}", files, format_bytes(bytes)),
        metrics: vec![
            Metric::new("files", files.to_string()),
            Metric::new("size", format_bytes(bytes)),
        ],
        actions: Vec::new(),
        link: Some("#/logs".to_string()),
    });

    // --- Updater ---
    if let Some(updater) = &state.updater {
        for img in updater.get_all_states().await {
            use crate::updater::UpdateStatus;
            let health = match img.status {
                UpdateStatus::Failed => Health::Error,
                UpdateStatus::Idle => Health::Ok,
                _ => Health::Warn,
            };
            let detail = match &img.last_update {
                Some(t) => format!(
                    "{:?} · updated {}",
                    img.status,
                    t.format("%Y-%m-%d %H:%M")
                )
                .to_lowercase(),
                None => format!("{:?} · never updated", img.status).to_lowercase(),
            };
            let digest = img
                .current_digest
                .as_deref()
                .map(|d| d.trim_start_matches("sha256:")[..12.min(d.len())].to_string())
                .unwrap_or_else(|| "-".to_string());
            out.push(ComponentSummary {
                id: format!("update:{}", img.image),
                kind: "updater".to_string(),
                label: img.image.clone(),
                health,
                detail,
                metrics: vec![Metric::new("digest", digest).tone("muted")],
                actions: vec![Action {
                    id: "trigger".to_string(),
                    label: "Update".to_string(),
                    method: "POST".to_string(),
                    path: format!("/api/v1/updates/{}/trigger", img.image),
                    enabled: img.status == UpdateStatus::Idle
                        || img.status == UpdateStatus::Failed,
                    danger: false,
                }],
                link: None,
            });
        }
    }

    out
}

async fn log_dir_totals(dir: &std::path::Path) -> (usize, u64) {
    let mut files = 0usize;
    let mut bytes = 0u64;
    if let Ok(mut entries) = tokio::fs::read_dir(dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if let Ok(meta) = entry.metadata().await {
                if meta.is_file() {
                    files += 1;
                    bytes += meta.len();
                }
            }
        }
    }
    (files, bytes)
}

pub fn format_duration(secs: i64) -> String {
    let secs = secs.max(0);
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    let s = secs % 60;
    if days > 0 {
        format!("{}d {}h", days, hours)
    } else if hours > 0 {
        format!("{}h {}m", hours, mins)
    } else if mins > 0 {
        format!("{}m {}s", mins, s)
    } else {
        format!("{}s", s)
    }
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} B", bytes)
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_formats_by_magnitude() {
        assert_eq!(format_duration(42), "42s");
        assert_eq!(format_duration(90), "1m 30s");
        assert_eq!(format_duration(3700), "1h 1m");
        assert_eq!(format_duration(90000), "1d 1h");
        assert_eq!(format_duration(-5), "0s");
    }

    #[test]
    fn bytes_format_by_magnitude() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2.0 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
    }
}
