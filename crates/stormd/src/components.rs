//! Assembling this stormd's component summaries — the one feed both
//! dashboards render from. The shapes themselves live in the shared
//! `stormview` crate (github.com/glennswest/stormview), the contract every
//! storm daemon serves and every storm UI renders; this module only knows
//! how THIS daemon's parts map onto it.

use crate::api::AppState;
use crate::supervisor::ProcessState;
use serde::Deserialize;

pub use stormview::{
    format_bytes, format_duration, Action, ComponentSummary, Health, Metric, Relation,
    RelationKind,
};

/// A start/stop/restart button on a process card.
fn process_action(id: &str, label: &str, process: &str, enabled: bool, danger: bool) -> Action {
    Action {
        id: id.to_string(),
        label: label.to_string(),
        method: "POST".to_string(),
        path: format!("/api/v1/processes/{}/{}", process, id),
        enabled,
        danger,
    }
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
        relations: Vec::new(),
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
            process_action("start", "Start", &p.name, stopped, false),
            process_action("stop", "Stop", &p.name, !stopped, true),
            process_action("restart", "Restart", &p.name, !stopped, false),
        ];

        out.push(ComponentSummary {
            id: format!("process:{}", p.name),
            kind: if plugin.is_some() { "plugin" } else { "process" }.to_string(),
            label: plugin.map(|u| u.label.clone()).unwrap_or_else(|| p.name.clone()),
            health,
            detail,
            metrics,
            actions,
            relations: Vec::new(),
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
            relations: Vec::new(),
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
            relations: Vec::new(),
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
        relations: Vec::new(),
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
                relations: Vec::new(),
                link: None,
            });
        }
    }

    // --- Relations ---
    // Wired as a pass over the finished list so every edge points at an id
    // that actually exists in this snapshot. The graph today: the system
    // has_many processes and mounts and has_one logs; everything belongs_to
    // the system; a tracked image has_one process and vice versa; a process
    // has_one logs view filtered to itself.
    let process_ids: Vec<String> = out
        .iter()
        .filter(|c| c.kind == "process" || c.kind == "plugin")
        .map(|c| c.id.clone())
        .collect();
    let mount_ids: Vec<String> = out
        .iter()
        .filter(|c| c.kind == "storage")
        .map(|c| c.id.clone())
        .collect();
    let update_ids: Vec<String> = out
        .iter()
        .filter(|c| c.kind == "updater")
        .map(|c| c.id.clone())
        .collect();
    let cron_ids: Vec<String> = out
        .iter()
        .filter(|c| c.kind == "cron")
        .map(|c| c.id.clone())
        .collect();

    for c in out.iter_mut() {
        match c.kind.as_str() {
            "system" => {
                if !process_ids.is_empty() {
                    c.relations.push(Relation::has_many("processes", process_ids.clone()));
                }
                if !cron_ids.is_empty() {
                    c.relations.push(Relation::has_many("cron", cron_ids.clone()));
                }
                if !mount_ids.is_empty() {
                    c.relations.push(Relation::has_many("storage", mount_ids.clone()));
                }
                c.relations.push(Relation::has_one("logs", "logs"));
            }
            "process" | "plugin" => {
                let name = c.id.strip_prefix("process:").unwrap_or(&c.id).to_string();
                c.relations.push(Relation::belongs_to("system", "system"));
                c.relations.push(
                    Relation::has_one("logs", "logs")
                        .href(format!("#/logs?process={}", name)),
                );
                let update_id = format!("update:{}", name);
                if update_ids.contains(&update_id) {
                    c.relations.push(Relation::has_one("update", update_id));
                }
            }
            "updater" => {
                let name = c.id.strip_prefix("update:").unwrap_or(&c.id).to_string();
                let process_id = format!("process:{}", name);
                if process_ids.contains(&process_id) {
                    c.relations.push(Relation::belongs_to("process", process_id));
                }
            }
            "cron" | "storage" | "logs" => {
                c.relations.push(Relation::belongs_to("system", "system"));
            }
            _ => {}
        }
    }

    // --- Plugin self-reported summaries ---
    // A plugin that publishes its own summary gets it merged into its card:
    // health and detail replace stormd's process-level view (the plugin knows
    // itself better), its metrics append after the supervisor's. Best-effort
    // with a short timeout, fetched concurrently — an absent endpoint costs
    // the card nothing but the extra detail.
    let with_summaries: Vec<_> = state
        .ui_plugins
        .iter()
        .filter_map(|p| p.summary_url.clone().map(|url| (p.name.clone(), url)))
        .collect();
    if !with_summaries.is_empty() {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(400))
            .build();
        if let Ok(client) = client {
            let fetches = with_summaries.into_iter().map(|(name, url)| {
                let client = client.clone();
                async move {
                    let remote = client
                        .get(&url)
                        .send()
                        .await
                        .ok()?
                        .json::<RemoteSummary>()
                        .await
                        .ok()?;
                    Some((name, remote))
                }
            });
            for fetched in futures_util::future::join_all(fetches).await.into_iter().flatten() {
                let (name, remote) = fetched;
                if let Some(card) = out.iter_mut().find(|c| c.id == format!("process:{}", name)) {
                    if let Some(health) = remote.health {
                        card.health = health;
                    }
                    if let Some(detail) = remote.detail {
                        card.detail = detail;
                    }
                    card.metrics.extend(remote.metrics);
                }
            }
        }
    }

    out
}

/// What a plugin's `summary` endpoint may return — every field optional.
#[derive(Debug, Deserialize)]
struct RemoteSummary {
    #[serde(default)]
    health: Option<Health>,
    #[serde(default)]
    detail: Option<String>,
    #[serde(default)]
    metrics: Vec<Metric>,
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
