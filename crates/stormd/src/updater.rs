use crate::config::{ProcessConfig, UpdaterConfig};
use crate::events::{EventBus, EventKind};
use crate::supervisor::Supervisor;
use chrono::{DateTime, Utc};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
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

/// What `pull_and_assemble` made: the rootfs, how to start it, and the
/// digest of the manifest (or index) that was pulled.
struct Assembled {
    rootfs: PathBuf,
    cmd: Vec<String>,
    env: HashMap<String, String>,
    working_dir: Option<PathBuf>,
    /// The identity of what is installed (stormd#82): the pull's own
    /// manifest digest, never a later lookup of a tag that may have moved.
    digest: String,
}

/// The manifest types a digest check asks for: the same list stormpull's
/// pull sends, so a check and a pull of an unchanged tag name the same
/// manifest or index (stormd#82). Asking for single manifests only, a
/// multi-arch tag answered with something other than what the pull recorded.
const MANIFEST_ACCEPT: &str = "application/vnd.oci.image.manifest.v1+json, \
     application/vnd.oci.image.index.v1+json, \
     application/vnd.docker.distribution.manifest.v2+json, \
     application/vnd.docker.distribution.manifest.list.v2+json";

pub struct Updater {
    config: UpdaterConfig,
    supervisor: Arc<Supervisor>,
    event_bus: Arc<EventBus>,
    state: RwLock<HashMap<String, ImageState>>,
    process_configs: Vec<ProcessConfig>,
    /// One update at a time per process (stormd#82): the poll and the
    /// trigger API share `<name>.new` and `<name>.old`, and two updates at
    /// once removed or pivoted each other's trees. Held from the pull to the
    /// cleanup of the old rootfs.
    update_locks: std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
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
            update_locks: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// The update lock of one process (see `update_locks`).
    fn update_lock(&self, name: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.update_locks.lock().unwrap_or_else(|e| e.into_inner());
        locks.entry(name.to_string()).or_default().clone()
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
            .header("Accept", MANIFEST_ACCEPT)
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

    /// Pull an image and assemble its rootfs. The caller holds the process's
    /// update lock.
    async fn pull_and_assemble(&self, process_name: &str, image_ref_str: &str) -> anyhow::Result<Assembled> {
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
            extract_layer(&layer_data, &new_rootfs)?;
        }

        info!(
            process = %process_name,
            rootfs = %new_rootfs.display(),
            cmd = ?entrypoint_cmd,
            "rootfs assembled"
        );

        Ok(Assembled {
            rootfs: new_rootfs,
            cmd: entrypoint_cmd,
            env: env_map,
            working_dir,
            digest: pulled.manifest_digest,
        })
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

        // The old rootfs goes inside the update, under its lock: a cleanup
        // left running in the background raced the next update's pivot over
        // the same `<name>.old` (stormd#82).
        if old_rootfs.exists() {
            if let Err(e) = tokio::fs::remove_dir_all(&old_rootfs).await {
                warn!(path = %old_rootfs.display(), error = %e, "failed to clean up old rootfs");
            }
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

        // A second update while one runs is refused, not queued: it would
        // pull the same tag again the moment the first finished.
        let lock = self.update_lock(process_name);
        let Ok(_update) = lock.try_lock() else {
            return Err(crate::goldens::refused(409, format!("an update of '{process_name}' is already running")));
        };
        self.update_locked(process_name, &image).await
    }

    /// Perform a full update cycle for one process, waiting for any update
    /// of it already running.
    async fn do_update(&self, process_name: &str, image_ref_str: &str) -> anyhow::Result<()> {
        let lock = self.update_lock(process_name);
        let _update = lock.lock().await;
        self.update_locked(process_name, image_ref_str).await
    }

    /// The update itself — pull, assemble, pivot, record, clean up. The
    /// caller holds the process's update lock (stormd#82).
    async fn update_locked(&self, process_name: &str, image_ref_str: &str) -> anyhow::Result<()> {
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

        let Assembled { rootfs: new_rootfs, cmd, env, working_dir, digest } = match self.pull_and_assemble(process_name, image_ref_str).await {
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

        // Success. What is installed is what was pulled: a tag that moved
        // during the pull or pivot is the next poll's update, not this one's
        // record (stormd#82). A post-pivot lookup of the tag recorded the new
        // digest over the old content, and the new release was never pulled.
        let new_digest = Some(digest);

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
                // An update of it already running (the trigger API) is this
                // round's check; the next poll looks again.
                let lock = self.update_lock(name);
                let Ok(_update) = lock.try_lock() else {
                    info!(process = %name, "update already running — skipping this poll");
                    continue;
                };
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

                if let Err(e) = self.update_locked(name, image).await {
                    error!(process = %name, error = %e, "update failed");
                }
            }
        }
    }
}

/// Extract one layer (tar or tar.gz) onto `root`, the rootfs assembled from
/// the layers below it.
///
/// **Whiteouts first, then the layer's own files** (stormd#82). A whiteout
/// says what to hide of the *lower* layers, so it is applied before anything
/// of this layer is written. Applied in tar order, an opaque marker that
/// followed its directory's new files deleted them with the old ones.
///
/// **Whiteouts stay inside the rootfs** (stormd#82). Unlike a file, which
/// `unpack_in` confines, a whiteout was a path built from the archive and
/// deleted directly: `../x/.wh.y`, an absolute path, or a parent an earlier
/// layer made a symlink reached outside the rootfs with stormd's privileges.
/// An absolute or `..` path now refuses the layer; a whiteout under a
/// symlinked parent is skipped with a warning, since what it names is not in
/// the rootfs.
fn extract_layer(data: &[u8], root: &Path) -> anyhow::Result<()> {
    fn archive(data: &[u8]) -> tar::Archive<Box<dyn Read + '_>> {
        let r: Box<dyn Read + '_> = if data.starts_with(&[0x1f, 0x8b]) { Box::new(GzDecoder::new(data)) } else { Box::new(data) };
        tar::Archive::new(r)
    }
    apply_whiteouts(&mut archive(data), root)?;
    for entry in archive(data).entries()? {
        let mut entry = entry?;
        if whiteout_name(&entry.path()?).is_some() {
            continue;
        }
        entry.unpack_in(root)?;
    }
    Ok(())
}

/// The file name of a whiteout entry (`.wh.<name>` or `.wh..wh..opq`).
fn whiteout_name(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    name.starts_with(".wh.").then(|| name.to_string())
}

/// An archive path as a path below the rootfs: refused if absolute or if any
/// part of it is `..`.
fn layer_relative(path: &Path) -> anyhow::Result<PathBuf> {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::Normal(n) => out.push(n),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                anyhow::bail!("layer entry '{}' leaves the rootfs — layer refused", path.display())
            }
        }
    }
    Ok(out)
}

/// `root/rel` when every part of it is a real directory: `Ok(None)` if part
/// of it does not exist (nothing to hide), `Err` naming the part that is a
/// symlink or not a directory.
fn real_dir(root: &Path, rel: &Path) -> Result<Option<PathBuf>, String> {
    let mut at = root.to_path_buf();
    for c in rel.components() {
        at.push(c);
        match std::fs::symlink_metadata(&at) {
            Ok(m) if m.file_type().is_symlink() => return Err(format!("{} is a symlink", at.display())),
            Ok(m) if !m.is_dir() => return Err(format!("{} is not a directory", at.display())),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("{}: {e}", at.display())),
        }
    }
    Ok(Some(at))
}

/// Remove a file, symlink or directory tree, never following a symlink.
fn remove_entry(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// A layer's whiteouts, applied to the lower layers already in `root`.
fn apply_whiteouts<R: Read>(archive: &mut tar::Archive<R>, root: &Path) -> anyhow::Result<()> {
    for entry in archive.entries()? {
        let entry = entry?;
        let path = entry.path()?.into_owned();
        let Some(name) = whiteout_name(&path) else { continue };
        let rel = layer_relative(&path)?;
        let parent = rel.parent().unwrap_or(Path::new(""));
        let dir = match real_dir(root, parent) {
            Ok(Some(d)) => d,
            Ok(None) => continue,
            Err(why) => {
                warn!(entry = %path.display(), reason = %why, "whiteout not inside the rootfs — skipped");
                continue;
            }
        };
        if name == ".wh..wh..opq" {
            // Opaque: nothing of the lower layers shows in this directory.
            for child in std::fs::read_dir(&dir)? {
                remove_entry(&child?.path())?;
            }
        } else if name.starts_with(".wh..wh.") {
            // Other `.wh..wh.` names are aufs metadata, not whiteouts.
            continue;
        } else {
            let hidden = name.strip_prefix(".wh.").unwrap_or_default();
            if hidden.is_empty() || hidden == "." || hidden == ".." {
                anyhow::bail!("whiteout '{}' names nothing — layer refused", path.display());
            }
            remove_entry(&dir.join(hidden))?;
        }
    }
    Ok(())
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

/// stormd#82 (10e643f2175d, 5bae04c3272c, 2d0e63dca0f9): whiteouts stay in
/// the rootfs and hide only the lower layers, whatever the tar order.
#[cfg(test)]
mod layer_tests {
    use super::extract_layer;
    use std::path::{Path, PathBuf};

    fn dir(label: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("stormd-layer-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("root")).unwrap();
        std::fs::create_dir_all(d.join("outside")).unwrap();
        d
    }

    /// A tar of `(name, Some(contents))` files and `(name, None)` dirs, the
    /// names written raw — `tar::Builder` refuses `..` and absolute paths,
    /// and these must reach the extractor as a hostile layer would send them.
    fn layer(entries: &[(&str, Option<&str>)]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for (name, body) in entries {
            let mut h = tar::Header::new_old();
            let data = body.unwrap_or("").as_bytes();
            h.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
            h.set_size(data.len() as u64);
            h.set_mode(if body.is_some() { 0o644 } else { 0o755 });
            h.set_entry_type(if body.is_some() { tar::EntryType::Regular } else { tar::EntryType::Directory });
            h.set_cksum();
            b.append(&h, data).unwrap();
        }
        b.into_inner().unwrap()
    }

    fn put(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn a_whiteout_leaving_the_rootfs_refuses_the_layer() {
        let d = dir("escape");
        let root = d.join("root");
        put(&d.join("outside/victim"), "keep");
        put(&root.join("etc/x"), "keep");
        let up = extract_layer(&layer(&[("../outside/.wh.victim", Some(""))]), &root);
        let abs = extract_layer(&layer(&[(&format!("{}/.wh.victim", d.join("outside").display()), Some(""))]), &root);
        let opq = extract_layer(&layer(&[("etc/../../outside/.wh..wh..opq", Some(""))]), &root);
        let kept = d.join("outside/victim").exists();
        let _ = std::fs::remove_dir_all(&d);
        assert!(up.unwrap_err().to_string().contains("leaves the rootfs"));
        assert!(abs.unwrap_err().to_string().contains("leaves the rootfs"));
        assert!(opq.unwrap_err().to_string().contains("leaves the rootfs"));
        assert!(kept, "a whiteout deleted outside the rootfs");
    }

    #[test]
    fn a_whiteout_under_a_symlinked_parent_is_skipped() {
        let d = dir("symlink");
        let root = d.join("root");
        put(&d.join("outside/victim"), "keep");
        put(&d.join("outside/other"), "keep");
        std::os::unix::fs::symlink(d.join("outside"), root.join("link")).unwrap();
        let r = extract_layer(&layer(&[("link/.wh.victim", Some("")), ("link/.wh..wh..opq", Some(""))]), &root);
        let kept = d.join("outside/victim").exists() && d.join("outside/other").exists();
        let _ = std::fs::remove_dir_all(&d);
        assert!(r.is_ok(), "{r:?}");
        assert!(kept, "a whiteout followed a symlink out of the rootfs");
    }

    #[test]
    fn an_opaque_marker_hides_only_the_lower_layers_in_either_order() {
        for (label, entries) in [
            ("marker-first", vec![("d/", None), ("d/.wh..wh..opq", Some("")), ("d/new", Some("layer"))]),
            ("marker-last", vec![("d/", None), ("d/new", Some("layer")), ("d/.wh..wh..opq", Some(""))]),
        ] {
            let d = dir(label);
            let root = d.join("root");
            put(&root.join("d/old"), "lower");
            put(&root.join("d/sub/deep"), "lower");
            extract_layer(&layer(&entries), &root).unwrap();
            let (new, old, sub) = (root.join("d/new").exists(), root.join("d/old").exists(), root.join("d/sub").exists());
            let _ = std::fs::remove_dir_all(&d);
            assert!(new, "{label}: the layer's own file was deleted");
            assert!(!old && !sub, "{label}: the lower layer still shows");
        }
    }

    #[test]
    fn a_whiteout_hides_a_lower_file_or_directory() {
        let d = dir("plain");
        let root = d.join("root");
        put(&root.join("a/f"), "lower");
        put(&root.join("a/dir/g"), "lower");
        put(&root.join("a/keep"), "lower");
        extract_layer(&layer(&[("a/.wh.f", Some("")), ("a/.wh.dir", Some("")), ("a/added", Some("x"))]), &root).unwrap();
        let state = (root.join("a/f").exists(), root.join("a/dir").exists(), root.join("a/keep").exists(), root.join("a/added").exists());
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(state, (false, false, true, true));
    }
}

/// stormd#82 (8125bda07bb1, ee990eabd358, 98cbfc29e7fd, a9f12a9b4236,
/// e686d162bda7): the digest recorded is the one pulled, the digest check
/// asks for the same manifest types as the pull, and one update runs at a
/// time per process.
#[cfg(test)]
mod update_tests {
    use super::{RootfsRecord, Updater};
    use axum::{extract::{Path as AxPath, State}, http::{HeaderMap, Method, StatusCode}, routing::get, Router};
    use sha2::{Digest, Sha256};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    fn digest(b: &[u8]) -> String {
        format!("sha256:{}", hex::encode(Sha256::digest(b)))
    }

    /// A registry whose tag moves the moment it has been pulled: GET answers
    /// manifest A, HEAD (any lookup after) answers digest B.
    #[derive(Default)]
    struct Registry {
        manifest: Vec<u8>,
        blobs: HashMap<String, Vec<u8>>,
        heads: Mutex<Vec<String>>,
    }

    const MOVED: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    async fn registry() -> (String, String, Arc<Registry>) {
        let mut b = tar::Builder::new(Vec::new());
        let mut h = tar::Header::new_gnu();
        h.set_size(5);
        h.set_mode(0o644);
        h.set_cksum();
        b.append_data(&mut h, "hello.txt", &b"hello"[..]).unwrap();
        let layer = b.into_inner().unwrap();
        let config = serde_json::to_vec(&serde_json::json!({
            "architecture": "amd64", "os": "linux",
            "config": { "Cmd": ["sleep", "30"] },
            "rootfs": { "type": "layers", "diff_ids": [digest(&layer)] },
            "history": [{ "created_by": "stormd-test" }],
        }))
        .unwrap();
        let manifest = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "config": { "mediaType": "application/vnd.oci.image.config.v1+json", "digest": digest(&config), "size": config.len() },
            "layers": [{ "mediaType": "application/vnd.oci.image.layer.v1.tar", "digest": digest(&layer), "size": layer.len() }],
        }))
        .unwrap();
        let pulled = digest(&manifest);
        let reg = Arc::new(Registry {
            manifest,
            blobs: HashMap::from([(digest(&layer), layer), (digest(&config), config)]),
            heads: Mutex::new(Vec::new()),
        });
        async fn serve_manifest(State(r): State<Arc<Registry>>, m: Method, h: HeaderMap) -> (HeaderMap, Vec<u8>) {
            let mut out = HeaderMap::new();
            if m == Method::HEAD {
                r.heads.lock().unwrap().push(h.get("accept").and_then(|v| v.to_str().ok()).unwrap_or("").to_string());
                out.insert("docker-content-digest", MOVED.parse().unwrap());
                return (out, Vec::new());
            }
            out.insert("docker-content-digest", digest(&r.manifest).parse().unwrap());
            out.insert("content-type", "application/vnd.oci.image.manifest.v1+json".parse().unwrap());
            (out, r.manifest.clone())
        }
        async fn blob(State(r): State<Arc<Registry>>, AxPath(d): AxPath<String>) -> Result<Vec<u8>, StatusCode> {
            r.blobs.get(&d).cloned().ok_or(StatusCode::NOT_FOUND)
        }
        let app = Router::new()
            .route("/v2/app/manifests/latest", get(serve_manifest))
            .route("/v2/app/blobs/{digest}", get(blob))
            .with_state(reg.clone());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let image = format!("{}/app:latest", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        (image, pulled, reg)
    }

    fn updater(label: &str, image: &str) -> (Arc<Updater>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("stormd-upd-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("rootfs")).unwrap();
        let cfg: crate::config::Config = toml::from_str(&format!(
            "[general]\nname = \"t\"\nlog_dir = \"{}\"\n[stormlog.mcast]\ngroup = \"off\"\n",
            dir.display()
        ))
        .unwrap();
        let mut log_cfg = cfg.stormlog.clone();
        log_cfg.file.log_dir = dir.clone();
        let bus = Arc::new(crate::events::EventBus::new(cfg.events.clone(), "t".into()));
        let log = Arc::new(stormlog::StormLog::new(log_cfg, "t"));
        let sup = Arc::new(crate::supervisor::Supervisor::new(log, bus.clone()));
        let p: crate::config::ProcessConfig =
            toml::from_str(&format!("name = \"app\"\ncommand = \"\"\nimage = \"{image}\"\n")).unwrap();
        let ucfg = crate::config::UpdaterConfig {
            enabled: true,
            poll_interval_secs: 3600,
            data_dir: dir.join("data"),
            rootfs_dir: dir.join("rootfs"),
        };
        (Arc::new(Updater::new(ucfg, sup, bus, vec![p])), dir)
    }

    #[tokio::test]
    async fn the_digest_recorded_is_the_one_pulled_not_where_the_tag_moved() {
        let (image, pulled, reg) = registry().await;
        let (u, dir) = updater("digest", &image);
        u.supervisor.register_process(u.process_configs[0].clone()).await;
        let r = u.do_update("app", &image).await;
        let rec = RootfsRecord::read(&dir.join("rootfs"), "app");
        let hello = dir.join("rootfs/app/hello.txt").exists();
        let check = u.check_digest(&stormpull::reference::ImageReference::parse(&image).unwrap()).await;
        let accept = reg.heads.lock().unwrap().clone();
        u.supervisor.stop_all().await;
        let _ = std::fs::remove_dir_all(&dir);

        assert!(r.is_ok(), "{r:?}");
        assert!(hello, "the layer was not assembled");
        assert_eq!(rec.and_then(|r| r.digest), Some(pulled), "recorded something other than what was pulled");
        assert_eq!(check.as_deref(), Some(MOVED), "the next check sees the moved tag — an update, not a no-op");
        assert_eq!(accept.len(), 1, "the update itself looked the tag up again: {accept:?}");
        assert!(accept[0].contains("image.index.v1+json") && accept[0].contains("manifest.list.v2+json"), "{accept:?}");
    }

    #[tokio::test]
    async fn a_second_update_of_a_process_is_refused_while_one_runs() {
        let (u, dir) = updater("lock", "127.0.0.1:1/app:latest");
        u.state.write().await.insert(
            "app".into(),
            super::ImageState {
                image: "127.0.0.1:1/app:latest".into(),
                current_digest: None,
                rootfs_path: None,
                last_check: None,
                last_update: None,
                status: super::UpdateStatus::Idle,
            },
        );
        let lock = u.update_lock("app");
        let held = lock.lock().await;
        let refused = u.trigger_update("app").await;
        drop(held);
        let _ = std::fs::remove_dir_all(&dir);
        let e = refused.unwrap_err();
        let status = e.downcast_ref::<crate::goldens::Refused>().map(|r| r.status);
        assert_eq!(status, Some(409), "{e}");
    }
}
