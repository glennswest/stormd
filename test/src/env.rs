//! What the runner hands the container (stormcentral `docs/test-standard.md`),
//! and where this run keeps its files.

use std::path::PathBuf;
use std::time::{Duration, Instant};

pub struct Env {
    pub suite: String,
    pub run_id: String,
    /// The node's address, for the read-only probe of its own stormds. Empty
    /// on a hand run.
    pub node: String,
    /// The stormd under test: `STORMD_BIN`, else `/stormd` (the image), else
    /// a `stormd` next to this binary (a cargo target directory).
    pub stormd: PathBuf,
    /// This binary, which every supervised process runs as `helper …`.
    pub me: PathBuf,
    /// Scratch for configs and stormd's logs: `/results/work` in the image,
    /// so a failure's logs come back with the results.
    pub work: PathBuf,
    pub results: PathBuf,
    started: Instant,
    timeout: Duration,
}

impl Env {
    pub fn read() -> Env {
        let var = |k: &str| std::env::var(k).unwrap_or_default();
        let me = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("/test"));
        let stormd = match std::env::var_os("STORMD_BIN") {
            Some(p) => PathBuf::from(p),
            None if std::path::Path::new("/stormd").exists() => PathBuf::from("/stormd"),
            None => me.with_file_name("stormd"),
        };
        let results = std::env::var_os("STORM_RESULTS")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/results"));
        let work = if results.is_dir() {
            results.join("work")
        } else {
            std::env::temp_dir().join(format!("stormd-test-{}", std::process::id()))
        };
        let suite = match var("STORM_SUITE") {
            s if s.is_empty() => "short".to_string(),
            s => s,
        };
        let default_timeout = match suite.as_str() {
            "short" => 120,
            "medium" => 1800,
            _ => 8 * 3600,
        };
        let timeout = var("STORM_TIMEOUT").parse().unwrap_or(default_timeout);
        let run_id = match var("STORM_RUN_ID") {
            s if s.is_empty() => format!("local-{}", std::process::id()),
            s => s,
        };
        Env {
            suite,
            run_id,
            node: var("STORM_NODE"),
            stormd,
            me,
            work,
            results,
            started: Instant::now(),
            timeout: Duration::from_secs(timeout),
        }
    }

    /// Time left of `STORM_TIMEOUT`.
    pub fn remaining(&self) -> Duration {
        self.timeout.saturating_sub(self.started.elapsed())
    }

    /// A fresh directory for one stormd instance.
    pub fn instance_dir(&self, label: &str) -> std::io::Result<PathBuf> {
        let d = self.work.join(label);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d)?;
        Ok(d)
    }
}
