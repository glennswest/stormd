use crate::config::{ProcessConfig, UpdaterConfig};
use crate::events::{EventBus, EventKind};
use crate::supervisor::Supervisor;
use chrono::{DateTime, Utc};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use stormpull::client::{PulledImage, RegistryClient};
use stormpull::reference::ImageReference;
use stormpull::store::BlobStore;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

#[derive(Debug, Clone, Serialize)]
pub struct ImageState {
    pub image: String,
    pub current_digest: Option<String>,
    pub rootfs_path: Option<PathBuf>,
    pub last_check: Option<DateTime<Utc>>,
    pub last_update: Option<DateTime<Utc>>,
    pub status: UpdateStatus,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum UpdateStatus {
    Idle,
    Checking,
    Pulling,
    Pivoting,
    Failed,
}

/// What the last pull and pivot started, kept beside the rootfs as
/// `<rootfs_dir>/<name>.image.json` (stormd#8).
///
/// **An image process whose rootfs existed was never started.** The command,
/// environment and working directory a pivot derives from the image config
/// lived only in memory, so after a stormd restart the updater found the
/// rootfs, had nothing to start it with, and left the process down until the
/// registry digest changed. It also recorded the *registry's* digest as the
/// current one, so an image published while stormd was down never arrived.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct RootfsRecord {
    image: String,
    digest: Option<String>,
    cmd: Vec<String>,
    env: HashMap<String, String>,
    working_dir: Option<PathBuf>,
}

impl RootfsRecord {
    fn path(rootfs_dir: &Path, name: &str) -> PathBuf {
        rootfs_dir.join(format!("{name}.image.json"))
    }

    fn read(rootfs_dir: &Path, name: &str) -> Option<RootfsRecord> {
        let text = std::fs::read_to_string(Self::path(rootfs_dir, name)).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Written to a temporary name and renamed, so a crash cannot leave half
    /// a record.
    fn write(&self, rootfs_dir: &Path, name: &str) -> std::io::Result<()> {
        let path = Self::path(rootfs_dir, name);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(tmp, path)
    }
}

/// The process config for an image process run from `rootfs`: the config
/// file's entry for the non-image fields, the image's entrypoint and
/// working directory resolved inside the rootfs (it is not chrooted), and the
/// image's env under the config's.
fn image_process_config(
    original: Option<&ProcessConfig>,
    name: &str,
    rootfs: &Path,
    cmd: &[String],
    env: &HashMap<String, String>,
    working_dir: Option<&Path>,
) -> anyhow::Result<ProcessConfig> {
    let (command, args) = cmd
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("image for '{name}' has no entrypoint or cmd"))?;
    let mut cfg = match original {
        Some(o) => o.clone(),
        // Shouldn't happen: every tracked process comes from the config.
        None => toml::from_str::<ProcessConfig>(&format!("name = {name:?}\ncommand = \"\"\n"))?,
    };
    cfg.command = match command.strip_prefix('/') {
        Some(rel) => rootfs.join(rel).to_string_lossy().to_string(),
        None => command.clone(),
    };
    cfg.args = args.to_vec();
    cfg.working_dir = Some(
        working_dir
            .map(|wd| rootfs.join(wd.strip_prefix("/").unwrap_or(wd)))
            .unwrap_or_else(|| rootfs.to_path_buf()),
    );
    let mut merged = env.clone();
    for (k, v) in &cfg.env {
        merged.insert(k.clone(), v.clone());
    }
    cfg.env = merged;
    Ok(cfg)
}

pub struct Updater {
    config: UpdaterConfig,
    supervisor: Arc<Supervisor>,
    event_bus: Arc<EventBus>,
    state: RwLock<HashMap<String, ImageState>>,
    process_configs: Vec<ProcessConfig>,
}

impl Updater {
    pub fn new(
        config: UpdaterConfig,
        supervisor: Arc<Supervisor>,
        event_bus: Arc<EventBus>,
        process_configs: Vec<ProcessConfig>,
    ) -> Self {
        Self {
            config,
            supervisor,
            event_bus,
            state: RwLock::new(HashMap::new()),
            process_configs,
        }
    }

    /// Get the update state for all tracked images.
    pub async fn get_all_states(&self) -> Vec<ImageState> {
        let state = self.state.read().await;
        state.values().cloned().collect()
    }

    /// Get the update state for a single process.
    pub async fn get_state(&self, name: &str) -> Option<ImageState> {
        let state = self.state.read().await;
        state.get(name).cloned()
    }

    /// Check the registry for a new digest without pulling.
    async fn check_digest(&self, image_ref: &ImageReference) -> Option<String> {
        let url = format!(
            "{}/v2/{}/manifests/{}",
            image_ref.registry_url(),
            image_ref.repository,
            image_ref.tag
        );

        let client = reqwest::Client::new();
        let resp = client
            .head(&url)
            .header(
                "Accept",
                "application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json",
            )
            .send()
            .await;

        match resp {
            Ok(r) if r.status().is_success() => {
                r.headers()
                    .get("docker-content-digest")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string())
            }
            Ok(r) => {
                warn!(
                    image = %image_ref.full_name(),
                    status = %r.status(),
                    "digest check returned non-success"
                );
                None
            }
            Err(e) => {
                warn!(
                    image = %image_ref.full_name(),
                    error = %e,
                    "registry unreachable for digest check"
                );
                None
            }
        }
    }

    /// Pull an image and assemble its rootfs. Returns (rootfs_path, entrypoint_command, env_vars).
    async fn pull_and_assemble(
        &self,
        process_name: &str,
        image_ref_str: &str,
    ) -> anyhow::Result<(PathBuf, Vec<String>, HashMap<String, String>, Option<PathBuf>)> {
        // Create blob store and registry client for this pull
        let store = BlobStore::new(&self.config.data_dir);
        store.init()?;

        let client = RegistryClient::new(store);
        let pulled: PulledImage = client.pull(image_ref_str).await?;

        info!(
            process = %process_name,
            manifest = %pulled.manifest_digest,
            config = %pulled.config_digest,
            layers = pulled.layer_digests.len(),
            "image pulled successfully"
        );

        // Read the config blob to extract CMD/ENTRYPOINT/ENV/WORKDIR
        let read_store = BlobStore::new(&self.config.data_dir);
        let config_bytes = read_store.get_blob(&pulled.config_digest)?;
        let oci_config: oci_spec::image::ImageConfiguration =
            serde_json::from_slice(&config_bytes)?;

        let mut entrypoint_cmd = Vec::new();
        let mut env_map = HashMap::new();
        let mut working_dir = None;

        if let Some(cfg) = oci_config.config() {
            if let Some(ep) = cfg.entrypoint() {
                entrypoint_cmd.extend(ep.iter().cloned());
            }
            if let Some(cmd) = cfg.cmd() {
                entrypoint_cmd.extend(cmd.iter().cloned());
            }
            if let Some(envs) = cfg.env() {
                for e in envs {
                    if let Some((k, v)) = e.split_once('=') {
                        env_map.insert(k.to_string(), v.to_string());
                    }
                }
            }
            if let Some(wd) = cfg.working_dir() {
                if !wd.is_empty() {
                    working_dir = Some(PathBuf::from(wd));
                }
            }
        }

        if entrypoint_cmd.is_empty() {
            anyhow::bail!(
                "image {} has no CMD or ENTRYPOINT",
                image_ref_str
            );
        }

        // Assemble rootfs by extracting all layers in order
        let new_rootfs = self.config.rootfs_dir.join(format!("{}.new", process_name));
        if new_rootfs.exists() {
            std::fs::remove_dir_all(&new_rootfs)?;
        }
        std::fs::create_dir_all(&new_rootfs)?;

        for (i, layer_digest) in pulled.layer_digests.iter().enumerate() {
            let layer_data = read_store.get_blob(layer_digest)?;
            info!(
                process = %process_name,
                layer = i,
                digest = %layer_digest,
                size = layer_data.len(),
                "extracting layer"
            );
            self.extract_layer(&layer_data, &new_rootfs)?;
        }

        info!(
            process = %process_name,
            rootfs = %new_rootfs.display(),
            cmd = ?entrypoint_cmd,
            "rootfs assembled"
        );

        Ok((new_rootfs, entrypoint_cmd, env_map, working_dir))
    }

    /// Extract a single layer (tar or tar.gz) to a target directory.
    /// Handles OCI whiteout files (.wh.) for layer deletions.
    fn extract_layer(&self, data: &[u8], target: &Path) -> anyhow::Result<()> {
        let is_gzip = data.len() >= 2 && data[0] == 0x1f && data[1] == 0x8b;

        if is_gzip {
            let decoder = GzDecoder::new(data);
            let mut archive = tar::Archive::new(decoder);
            self.unpack_archive(&mut archive, target)?;
        } else {
            let mut archive = tar::Archive::new(data);
            self.unpack_archive(&mut archive, target)?;
        }

        Ok(())
    }

    /// Unpack a tar archive handling OCI whiteout markers.
    fn unpack_archive<R: Read>(&self, archive: &mut tar::Archive<R>, target: &Path) -> anyhow::Result<()> {
        for entry in archive.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.into_owned();
            let file_name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            if file_name == ".wh..wh..opq" {
                // Opaque whiteout — clear entire directory
                let parent = target.join(path.parent().unwrap_or(Path::new("")));
                if parent.exists() {
                    for child in std::fs::read_dir(&parent)? {
                        let child = child?;
                        let ct = child.file_type()?;
                        if ct.is_dir() {
                            std::fs::remove_dir_all(child.path())?;
                        } else {
                            std::fs::remove_file(child.path())?;
                        }
                    }
                }
                continue;
            }

            if file_name.starts_with(".wh.") {
                // Regular whiteout — delete the target file/dir
                let original_name = &file_name[4..];
                let target_path = target.join(path.parent().unwrap_or(Path::new(""))).join(original_name);
                if target_path.is_dir() {
                    let _ = std::fs::remove_dir_all(&target_path);
                } else {
                    let _ = std::fs::remove_file(&target_path);
                }
                continue;
            }

            // Normal file — extract to target
            entry.unpack_in(target)?;
        }

        Ok(())
    }

    /// Stop old process, swap rootfs dirs, update config, start new process.
    async fn pivot(
        &self,
        process_name: &str,
        new_rootfs: PathBuf,
        cmd: Vec<String>,
        env: HashMap<String, String>,
        working_dir: Option<PathBuf>,
    ) -> anyhow::Result<()> {
        let current_rootfs = self.config.rootfs_dir.join(process_name);
        let old_rootfs = self.config.rootfs_dir.join(format!("{}.old", process_name));

        // Stop process — it may not be running yet on initial pull
        match self.supervisor.stop_process(process_name).await {
            Ok(()) => {
                // Wait for it to be gone: SIGTERM, then its stop timeout.
                self.supervisor.wait_stopped(process_name).await;
            }
            Err(_) => {
                // Process wasn't running — that's fine for initial pull
            }
        }

        // Clean up old rootfs from previous pivot
        if old_rootfs.exists() {
            std::fs::remove_dir_all(&old_rootfs)?;
        }

        // Swap: current → old, new → current
        if current_rootfs.exists() {
            std::fs::rename(&current_rootfs, &old_rootfs)?;
        }
        std::fs::rename(&new_rootfs, &current_rootfs)?;

        let original = self.process_configs.iter().find(|p| p.name == process_name);
        let updated_config = image_process_config(
            original,
            process_name,
            &current_rootfs,
            &cmd,
            &env,
            working_dir.as_deref(),
        )?;

        self.supervisor
            .update_process_config(process_name, updated_config)
            .await?;

        self.supervisor.start_process(process_name).await?;

        // Spawn background cleanup of old rootfs
        if old_rootfs.exists() {
            tokio::spawn(async move {
                if let Err(e) = tokio::fs::remove_dir_all(&old_rootfs).await {
                    warn!(path = %old_rootfs.display(), error = %e, "failed to clean up old rootfs");
                }
            });
        }

        Ok(())
    }

    /// Start an image process from its existing rootfs, as the last pivot
    /// left it.
    async fn start_recorded(&self, name: &str, rootfs: &Path, rec: &RootfsRecord) -> anyhow::Result<()> {
        let original = self.process_configs.iter().find(|p| p.name == name);
        let cfg = image_process_config(original, name, rootfs, &rec.cmd, &rec.env, rec.working_dir.as_deref())?;
        self.supervisor.update_process_config(name, cfg).await?;
        self.supervisor.start_process(name).await
    }

    /// Force immediate update check + pull + pivot for a single process.
    pub async fn trigger_update(self: &Arc<Self>, process_name: &str) -> anyhow::Result<()> {
        let image = {
            let state = self.state.read().await;
            match state.get(process_name) {
                Some(s) => s.image.clone(),
                None => anyhow::bail!("process '{}' is not tracked by updater", process_name),
            }
        };

        self.do_update(process_name, &image).await
    }

    /// Perform a full update cycle for one process.
    async fn do_update(&self, process_name: &str, image_ref_str: &str) -> anyhow::Result<()> {
        let detail = |msg: &str| {
            let mut d = HashMap::new();
            d.insert("image".to_string(), serde_json::Value::String(image_ref_str.to_string()));
            d.insert("detail".to_string(), serde_json::Value::String(msg.to_string()));
            d
        };

        // Pull
        {
            let mut state = self.state.write().await;
            if let Some(s) = state.get_mut(process_name) {
                s.status = UpdateStatus::Pulling;
            }
        }

        self.event_bus
            .emit(
                EventKind::UpdatePulling,
                Some(process_name.to_string()),
                detail("pulling image"),
            )
            .await;

        let (new_rootfs, cmd, env, working_dir) = match self.pull_and_assemble(process_name, image_ref_str).await {
            Ok(r) => r,
            Err(e) => {
                error!(process = %process_name, error = %e, "pull failed");
                let mut state = self.state.write().await;
                if let Some(s) = state.get_mut(process_name) {
                    s.status = UpdateStatus::Failed;
                }
                self.event_bus
                    .emit(
                        EventKind::UpdateFailed,
                        Some(process_name.to_string()),
                        detail(&format!("pull failed: {}", e)),
                    )
                    .await;
                return Err(e);
            }
        };

        // Pivot
        {
            let mut state = self.state.write().await;
            if let Some(s) = state.get_mut(process_name) {
                s.status = UpdateStatus::Pivoting;
            }
        }

        self.event_bus
            .emit(
                EventKind::UpdatePivoting,
                Some(process_name.to_string()),
                detail("pivoting rootfs"),
            )
            .await;

        let record_parts = (cmd.clone(), env.clone(), working_dir.clone());
        if let Err(e) = self.pivot(process_name, new_rootfs, cmd, env, working_dir).await {
            error!(process = %process_name, error = %e, "pivot failed");
            let mut state = self.state.write().await;
            if let Some(s) = state.get_mut(process_name) {
                s.status = UpdateStatus::Failed;
            }
            self.event_bus
                .emit(
                    EventKind::UpdateFailed,
                    Some(process_name.to_string()),
                    detail(&format!("pivot failed: {}", e)),
                )
                .await;
            return Err(e);
        }

        // Success — update state
        let image_ref = ImageReference::parse(image_ref_str).ok();
        let new_digest = if let Some(ref img) = image_ref {
            self.check_digest(img).await
        } else {
            None
        };

        let (cmd, env, working_dir) = record_parts;
        let record = RootfsRecord {
            image: image_ref_str.to_string(),
            digest: new_digest.clone(),
            cmd,
            env,
            working_dir,
        };
        if let Err(e) = record.write(&self.config.rootfs_dir, process_name) {
            warn!(process = %process_name, error = %e, "could not record the rootfs — a stormd restart will pull it again");
        }

        {
            let mut state = self.state.write().await;
            if let Some(s) = state.get_mut(process_name) {
                s.current_digest = new_digest;
                s.rootfs_path = Some(self.config.rootfs_dir.join(process_name));
                s.last_update = Some(Utc::now());
                s.status = UpdateStatus::Idle;
            }
        }

        self.event_bus
            .emit(
                EventKind::UpdateCompleted,
                Some(process_name.to_string()),
                detail("update completed"),
            )
            .await;

        info!(process = %process_name, image = %image_ref_str, "update completed");
        Ok(())
    }

    /// Main poll loop — checks for updates on all tracked images.
    pub async fn poll_loop(self: Arc<Self>) {
        // Ensure directories exist
        if let Err(e) = std::fs::create_dir_all(&self.config.data_dir) {
            error!(error = %e, path = %self.config.data_dir.display(), "failed to create data_dir");
            return;
        }
        if let Err(e) = std::fs::create_dir_all(&self.config.rootfs_dir) {
            error!(error = %e, path = %self.config.rootfs_dir.display(), "failed to create rootfs_dir");
            return;
        }

        // Initialize state for all image-tracked processes
        let tracked: Vec<(String, String)> = self
            .process_configs
            .iter()
            .filter_map(|p| p.image.as_ref().map(|img| (p.name.clone(), img.clone())))
            .collect();

        if tracked.is_empty() {
            info!("updater: no processes with image tracking configured");
            return;
        }

        {
            let mut state = self.state.write().await;
            for (name, image) in &tracked {
                state.insert(
                    name.clone(),
                    ImageState {
                        image: image.clone(),
                        current_digest: None,
                        rootfs_path: None,
                        last_check: None,
                        last_update: None,
                        status: UpdateStatus::Idle,
                    },
                );
            }
        }

        info!(
            count = tracked.len(),
            interval_secs = self.config.poll_interval_secs,
            "updater started"
        );

        // Start each process: from its rootfs and what the last pivot
        // recorded, or by pulling when there is no rootfs (or no record of
        // how to start it).
        for (name, image) in &tracked {
            // Register the process in supervisor if not already there
            if self.supervisor.get_status(name).await.is_err() {
                if let Some(cfg) = self.process_configs.iter().find(|p| p.name == *name) {
                    self.supervisor.register_process(cfg.clone()).await;
                }
            }
            let rootfs = self.config.rootfs_dir.join(name);
            let record = if rootfs.exists() { RootfsRecord::read(&self.config.rootfs_dir, name) } else { None };
            match record {
                Some(rec) => {
                    info!(process = %name, digest = ?rec.digest, "starting from the existing rootfs");
                    if let Err(e) = self.start_recorded(name, &rootfs, &rec).await {
                        error!(process = %name, error = %e, "could not start from the existing rootfs — pulling again");
                        if let Err(e) = self.do_update(name, image).await {
                            error!(process = %name, error = %e, "pull failed");
                        }
                        continue;
                    }
                    let mut state = self.state.write().await;
                    if let Some(s) = state.get_mut(name) {
                        // The rootfs's own digest: the first poll pulls a
                        // newer one if the registry has it.
                        s.current_digest = rec.digest.clone();
                        s.rootfs_path = Some(rootfs);
                        s.last_check = Some(Utc::now());
                    }
                }
                None => {
                    if rootfs.exists() {
                        info!(process = %name, image = %image, "rootfs has no record of how to start it — pulling again");
                    } else {
                        info!(process = %name, image = %image, "initial pull — no rootfs exists");
                    }
                    if let Err(e) = self.do_update(name, image).await {
                        error!(process = %name, error = %e, "initial pull failed");
                    }
                }
            }
        }

        // Poll loop
        let interval = tokio::time::Duration::from_secs(self.config.poll_interval_secs);
        loop {
            tokio::time::sleep(interval).await;

            for (name, image) in &tracked {
                let image_ref = match ImageReference::parse(image) {
                    Ok(r) => r,
                    Err(e) => {
                        warn!(process = %name, image = %image, error = %e, "invalid image reference");
                        continue;
                    }
                };

                // Update last check time
                {
                    let mut state = self.state.write().await;
                    if let Some(s) = state.get_mut(name) {
                        s.last_check = Some(Utc::now());
                        s.status = UpdateStatus::Checking;
                    }
                }

                self.event_bus
                    .emit_simple(EventKind::UpdateCheckStarted, Some(name.clone()))
                    .await;

                let new_digest = match self.check_digest(&image_ref).await {
                    Some(d) => d,
                    None => {
                        let mut state = self.state.write().await;
                        if let Some(s) = state.get_mut(name) {
                            s.status = UpdateStatus::Idle;
                        }
                        continue;
                    }
                };

                // Check if digest changed
                let needs_update = {
                    let state = self.state.read().await;
                    state
                        .get(name)
                        .map(|s| s.current_digest.as_deref() != Some(&new_digest))
                        .unwrap_or(true)
                };

                if !needs_update {
                    let mut state = self.state.write().await;
                    if let Some(s) = state.get_mut(name) {
                        s.status = UpdateStatus::Idle;
                    }
                    continue;
                }

                info!(
                    process = %name,
                    image = %image,
                    new_digest = %new_digest,
                    "new image version detected"
                );

                self.event_bus
                    .emit_simple(EventKind::UpdateAvailable, Some(name.clone()))
                    .await;

                if let Err(e) = self.do_update(name, image).await {
                    error!(process = %name, error = %e, "update failed");
                }
            }
        }
    }
}

#[cfg(test)]
mod record_tests {
    use super::{image_process_config, RootfsRecord};
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    #[test]
    fn a_record_round_trips_and_a_missing_one_is_none() {
        let dir = std::env::temp_dir().join(format!("stormd-record-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rec = RootfsRecord {
            image: "registry.g8.lo/app:latest".into(),
            digest: Some("sha256:abc".into()),
            cmd: vec!["/usr/bin/app".into(), "--serve".into()],
            env: HashMap::from([("A".to_string(), "1".to_string())]),
            working_dir: Some(PathBuf::from("/srv")),
        };
        rec.write(&dir, "app").unwrap();
        let back = RootfsRecord::read(&dir, "app");
        let none = RootfsRecord::read(&dir, "other");
        std::fs::write(dir.join("bad.image.json"), b"{not json").unwrap();
        let bad = RootfsRecord::read(&dir, "bad");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(back, Some(rec));
        assert_eq!((none, bad), (None, None));
    }

    /// The same config a pivot builds: entrypoint and workdir inside the
    /// rootfs, config env over image env, the config's other fields kept.
    #[test]
    fn the_config_runs_inside_the_rootfs() {
        let orig: crate::config::ProcessConfig = toml::from_str(
            "name = \"app\"\ncommand = \"\"\nimage = \"r/app:1\"\nenv = { A = \"config\" }\nrestart_delay_secs = 7\n",
        )
        .unwrap();
        let env = HashMap::from([("A".to_string(), "image".to_string()), ("B".to_string(), "image".to_string())]);
        let root = Path::new("/data/rootfs/app");
        let cmd = ["/usr/bin/app".to_string(), "--serve".to_string()];
        let c = image_process_config(Some(&orig), "app", root, &cmd, &env, Some(Path::new("/srv"))).unwrap();
        assert_eq!(c.command, "/data/rootfs/app/usr/bin/app");
        assert_eq!(c.args, vec!["--serve"]);
        assert_eq!(c.working_dir, Some(PathBuf::from("/data/rootfs/app/srv")));
        assert_eq!((c.env["A"].as_str(), c.env["B"].as_str()), ("config", "image"));
        assert_eq!(c.restart_delay_secs, 7);

        let c = image_process_config(None, "app", root, &["app".to_string()], &HashMap::new(), None).unwrap();
        assert_eq!((c.command.as_str(), c.working_dir), ("app", Some(root.to_path_buf())));
        assert!(image_process_config(None, "app", root, &[], &HashMap::new(), None).is_err(), "no entrypoint");
    }
}
