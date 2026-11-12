//! Goldens a process names, attached read-only and presented to it
//! (stormd#36, minismbd#11 option A).
//!
//! A process lists goldens (`[[process.golden]]`). Before it first starts,
//! stormd asks the node's stormblock engine for a read-only ublk attach of
//! each, makes the device node (a container's `/dev` is a tmpfs without it),
//! and presents it: a `filesystem` golden is mounted read-only at its path, an
//! `image` golden is the device node itself at its path, owned so the service
//! can read it. The service serves plain paths and never talks to stormblock.
//!
//! stormd runs as root in its container with its capabilities (stormpump
//! drops none), and the mounts land in the container's own mount namespace:
//! visible to its processes, not to the host.

use crate::config::{GoldenContent, GoldenMount, GoldensConfig};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// What stormd does to the container's filesystem. Behind a trait so the
/// sequence can be tested without root.
pub trait Host: Send + Sync {
    /// The device number of a block device the engine named (`/dev/ublkbN`):
    /// from the node if it exists, else `/sys/block/<name>/dev`.
    fn devno(&self, device: &Path) -> std::io::Result<u64>;
    /// Create (or replace) a block device node.
    fn make_node(&self, path: &Path, devno: u64, mode: u32, owner: Option<(u32, u32)>) -> std::io::Result<()>;
    fn mkdir_p(&self, path: &Path) -> std::io::Result<()>;
    fn mount_ro(&self, device: &Path, path: &Path, fstype: &str) -> std::io::Result<()>;
    fn unmount(&self, path: &Path) -> std::io::Result<()>;
    fn remove(&self, path: &Path) -> std::io::Result<()>;
}

/// The real thing: mknod, mount(2), umount(2).
pub struct RealHost;

#[cfg(target_os = "linux")]
impl Host for RealHost {
    fn devno(&self, device: &Path) -> std::io::Result<u64> {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        if let Ok(m) = std::fs::metadata(device) {
            if m.file_type().is_block_device() {
                return Ok(m.rdev());
            }
        }
        let name = device
            .file_name()
            .ok_or_else(|| std::io::Error::other(format!("{} names no device", device.display())))?;
        let sys = Path::new("/sys/block").join(name).join("dev");
        let text = std::fs::read_to_string(&sys)?;
        let (maj, min) = text
            .trim()
            .split_once(':')
            .ok_or_else(|| std::io::Error::other(format!("{}: not maj:min", sys.display())))?;
        let maj: u64 = maj.parse().map_err(std::io::Error::other)?;
        let min: u64 = min.parse().map_err(std::io::Error::other)?;
        Ok(nix::sys::stat::makedev(maj, min))
    }

    fn make_node(&self, path: &Path, devno: u64, mode: u32, owner: Option<(u32, u32)>) -> std::io::Result<()> {
        use nix::sys::stat::{mknod, Mode, SFlag};
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        let same = std::fs::metadata(path)
            .map(|m| m.file_type().is_block_device() && m.rdev() == devno)
            .unwrap_or(false);
        if !same {
            let _ = std::fs::remove_file(path);
            mknod(path, SFlag::S_IFBLK, Mode::from_bits_truncate(mode), devno)?;
        }
        // mknod applies the umask; say the mode outright.
        std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(mode))?;
        if let Some((uid, gid)) = owner {
            nix::unistd::chown(path, Some(uid.into()), Some(gid.into()))?;
        }
        Ok(())
    }

    fn mkdir_p(&self, path: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(path)
    }

    fn mount_ro(&self, device: &Path, path: &Path, fstype: &str) -> std::io::Result<()> {
        use nix::mount::{mount, MsFlags};
        mount(
            Some(device),
            path,
            Some(fstype),
            MsFlags::MS_RDONLY | MsFlags::MS_NODEV | MsFlags::MS_NOSUID,
            None::<&str>,
        )?;
        Ok(())
    }

    fn unmount(&self, path: &Path) -> std::io::Result<()> {
        nix::mount::umount(path)?;
        Ok(())
    }

    fn remove(&self, path: &Path) -> std::io::Result<()> {
        match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

#[cfg(not(target_os = "linux"))]
impl Host for RealHost {
    fn devno(&self, _: &Path) -> std::io::Result<u64> { Err(std::io::Error::other("linux only")) }
    fn make_node(&self, _: &Path, _: u64, _: u32, _: Option<(u32, u32)>) -> std::io::Result<()> { Err(std::io::Error::other("linux only")) }
    fn mkdir_p(&self, p: &Path) -> std::io::Result<()> { std::fs::create_dir_all(p) }
    fn mount_ro(&self, _: &Path, _: &Path, _: &str) -> std::io::Result<()> { Err(std::io::Error::other("linux only")) }
    fn unmount(&self, _: &Path) -> std::io::Result<()> { Err(std::io::Error::other("linux only")) }
    fn remove(&self, p: &Path) -> std::io::Result<()> { let _ = std::fs::remove_file(p); Ok(()) }
}

/// A golden as presented, for the API.
#[derive(Debug, Clone, Serialize)]
pub struct Presented {
    pub process: String,
    pub name: String,
    pub golden: Option<String>,
    pub volume_id: String,
    pub content: GoldenContent,
    pub device: String,
    pub path: String,
    pub size_bytes: Option<u64>,
    /// Added at runtime through the API (stormd#73), not from the config.
    pub added: bool,
    /// The golden as asked for, to tell a re-add of the same from a change.
    #[serde(skip)]
    pub spec: GoldenMount,
}

/// A request stormd will not carry out, with the HTTP status that says why
/// (400 a bad golden, 404 nothing by that name, 409 a conflict).
#[derive(Debug)]
pub struct Refused {
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Refused {}

pub fn refused(status: u16, message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(Refused { status, message: message.into() })
}

/// What an add did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Added {
    /// Newly presented (201).
    Created,
    /// Already presented with another spec: the old released, the new presented.
    Replaced,
    /// Already presented exactly so: nothing done.
    Unchanged,
}

pub struct Goldens {
    cfg: GoldensConfig,
    container: String,
    host: Arc<dyn Host>,
    client: reqwest::Client,
    presented: Mutex<HashMap<(String, String), Presented>>,
    /// Runtime adds and removes one at a time (stormd#73).
    ops: Mutex<()>,
}

impl Goldens {
    pub fn new(cfg: GoldensConfig, container: String, host: Arc<dyn Host>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_default();
        Self { cfg, container, host, client, presented: Mutex::new(HashMap::new()), ops: Mutex::new(()) }
    }

    fn engine(&self) -> String {
        let vars = crate::nodevars::vars();
        crate::nodevars::expand(&self.cfg.engine_url, &vars).trim_end_matches('/').to_string()
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match std::fs::read_to_string(&self.cfg.token_file) {
            Ok(t) if !t.trim().is_empty() => req.bearer_auth(t.trim()),
            _ => req,
        }
    }

    /// The path a golden appears at.
    pub fn path_of(&self, g: &GoldenMount) -> PathBuf {
        match &g.path {
            Some(p) => PathBuf::from(p),
            None => self.cfg.dir.join(&g.name),
        }
    }

    /// Who holds the attach, so the engine can say and release only ours.
    fn holder(&self, process: &str, name: &str) -> String {
        format!("stormd/{}/{}/{}", self.container, process, name)
    }

    async fn resolve(&self, g: &GoldenMount) -> anyhow::Result<String> {
        if let Some(id) = &g.volume_id {
            return Ok(id.clone());
        }
        let want = g.golden.as_deref().unwrap_or_default();
        let url = format!("{}/api/v1/volumes?kind=golden", self.engine());
        let resp = self.auth(self.client.get(&url)).send().await?;
        let status = resp.status();
        if !status.is_success() {
            anyhow::bail!("{url}: {status}: {}", resp.text().await.unwrap_or_default());
        }
        let body: serde_json::Value = resp.json().await?;
        body["items"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|v| v["name"] == want)
            .and_then(|v| v["id"].as_str().map(String::from))
            .ok_or_else(|| anyhow::anyhow!("no golden named {want} on the engine"))
    }

    async fn attach(&self, volume_id: &str, holder: &str) -> anyhow::Result<PathBuf> {
        let url = format!("{}/api/v1/volumes/{volume_id}/attach", self.engine());
        let body = serde_json::json!({ "mode": "ro", "transport": "ublk", "holder": holder });
        let resp = self.auth(self.client.post(&url).json(&body)).send().await?;
        let status = resp.status();
        if !status.is_success() {
            anyhow::bail!("{url}: {status}: {}", resp.text().await.unwrap_or_default());
        }
        let info: serde_json::Value = resp.json().await?;
        match (info["transport"].as_str(), info["device_hint"].as_str()) {
            (Some("ublk"), Some(dev)) => Ok(PathBuf::from(dev)),
            _ => anyhow::bail!("{url}: not a local ublk attach: {info}"),
        }
    }

    async fn detach(&self, volume_id: &str, holder: &str) -> anyhow::Result<()> {
        let url = format!("{}/api/v1/volumes/{volume_id}/attach", self.engine());
        let resp = self.auth(self.client.delete(&url).query(&[("holder", holder)])).send().await?;
        let status = resp.status();
        if !status.is_success() && status != reqwest::StatusCode::NOT_FOUND {
            anyhow::bail!("{url}: {status}: {}", resp.text().await.unwrap_or_default());
        }
        Ok(())
    }

    /// Attach and present one golden. On a failure after the attach, the
    /// attach is released again.
    pub async fn present(&self, process: &str, g: &GoldenMount) -> anyhow::Result<Presented> {
        self.present_as(process, g, false).await
    }

    async fn present_as(&self, process: &str, g: &GoldenMount, added: bool) -> anyhow::Result<Presented> {
        let volume_id = self.resolve(g).await?;
        let holder = self.holder(process, &g.name);
        let device = self.attach(&volume_id, &holder).await?;
        let path = self.path_of(g);
        if let Err(e) = self.place(g, &device, &path) {
            let _ = self.detach(&volume_id, &holder).await;
            return Err(e);
        }
        let p = Presented {
            process: process.to_string(),
            name: g.name.clone(),
            golden: g.golden.clone(),
            volume_id,
            content: g.content,
            device: device.display().to_string(),
            path: path.display().to_string(),
            size_bytes: g.size_bytes,
            added,
            spec: g.clone(),
        };
        info!(process, added, golden = %g.name, volume = %p.volume_id, device = %p.device, path = %p.path, "golden presented read-only");
        self.presented.lock().await.insert((process.to_string(), g.name.clone()), p.clone());
        Ok(p)
    }

    fn place(&self, g: &GoldenMount, device: &Path, path: &Path) -> anyhow::Result<()> {
        let devno = self.host.devno(device)?;
        match g.content {
            GoldenContent::Filesystem => {
                // The node the mount reads from, in the container's own /dev.
                self.host.make_node(device, devno, 0o400, None)?;
                self.host.mkdir_p(path)?;
                self.host.mount_ro(device, path, &g.fstype)?;
            }
            GoldenContent::Image => {
                self.host.make_node(path, devno, g.mode, g.owner_ids())?;
            }
        }
        Ok(())
    }

    /// Undo `present`: unmount (filesystem) or remove the node (image), then
    /// release the attach. Nothing presented under that name is not an error.
    pub async fn release(&self, process: &str, name: &str) -> anyhow::Result<()> {
        let Some(p) = self.presented.lock().await.remove(&(process.to_string(), name.to_string())) else {
            return Ok(());
        };
        let path = PathBuf::from(&p.path);
        let undone = match p.content {
            GoldenContent::Filesystem => self.host.unmount(&path),
            GoldenContent::Image => self.host.remove(&path),
        };
        if let Err(e) = undone {
            // Still mounted: the engine would refuse the detach (409) anyway.
            self.presented.lock().await.insert((process.to_string(), name.to_string()), p);
            anyhow::bail!("{}: {e}", path.display());
        }
        self.detach(&p.volume_id, &self.holder(process, name)).await?;
        info!(process, golden = %name, volume = %p.volume_id, "golden released");
        Ok(())
    }

    /// Present every golden of a process, retrying each every 2 s until it
    /// works or `stop()` says to give up (shutdown). One warning per
    /// distinct error, not one per try.
    pub async fn present_all(&self, process: &str, goldens: &[GoldenMount], stop: impl Fn() -> bool) -> bool {
        for g in goldens {
            if self.presented.lock().await.contains_key(&(process.to_string(), g.name.clone())) {
                continue;
            }
            let mut last = String::new();
            loop {
                if stop() {
                    return false;
                }
                match self.present(process, g).await {
                    Ok(_) => break,
                    Err(e) => {
                        let e = e.to_string();
                        if e != last {
                            warn!(process, golden = %g.name, error = %e, "golden not presented — retrying every 2 s");
                            last = e;
                        }
                    }
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
        true
    }

    /// Add a golden to a running process without stopping it (stormd#73).
    /// `reserved` are the paths the config's goldens use, presented or not.
    /// The same spec again does nothing; another spec under the same name
    /// replaces it, and the old one is put back if the new one fails. A
    /// failed add leaves nothing attached (`present` detaches on failure).
    pub async fn add(&self, process: &str, g: GoldenMount, reserved: &[PathBuf]) -> anyhow::Result<(Added, Presented)> {
        g.validate().map_err(|e| refused(400, e))?;
        let _op = self.ops.lock().await;
        let key = (process.to_string(), g.name.clone());
        let path = self.path_of(&g);
        let old = {
            let presented = self.presented.lock().await;
            let old = presented.get(&key).cloned();
            if let Some(old) = &old {
                if !old.added {
                    return Err(refused(409, format!("golden '{}' of '{process}' is from the config", g.name)));
                }
                if old.spec == g {
                    return Ok((Added::Unchanged, old.clone()));
                }
            }
            if let Some(other) = presented.values().find(|p| Path::new(&p.path) == path && (p.process != process || p.name != g.name)) {
                return Err(refused(409, format!("{} is already golden '{}' of '{}'", path.display(), other.name, other.process)));
            }
            old
        };
        if reserved.iter().any(|r| *r == path) {
            return Err(refused(409, format!("{} is a configured golden's path", path.display())));
        }
        let Some(old) = old else {
            return Ok((Added::Created, self.present_as(process, &g, true).await?));
        };
        self.release(process, &g.name).await?;
        match self.present_as(process, &g, true).await {
            Ok(p) => Ok((Added::Replaced, p)),
            Err(e) => {
                warn!(process, golden = %g.name, error = %e, "new golden not presented — putting the old one back");
                if let Err(e2) = self.present_as(process, &old.spec, true).await {
                    warn!(process, golden = %g.name, error = %e2, "old golden not presented either");
                }
                Err(e)
            }
        }
    }

    /// Remove a golden added at runtime, without stopping the process
    /// (stormd#73). A configured one is refused (409), an unknown one 404.
    pub async fn remove(&self, process: &str, name: &str) -> anyhow::Result<()> {
        let _op = self.ops.lock().await;
        match self.presented.lock().await.get(&(process.to_string(), name.to_string())) {
            None => return Err(refused(404, format!("process '{process}' has no golden '{name}'"))),
            Some(p) if !p.added => {
                return Err(refused(409, format!("golden '{name}' of '{process}' is from the config")))
            }
            Some(_) => {}
        }
        self.release(process, name).await
    }

    /// Release everything, at shutdown.
    pub async fn release_all(&self) {
        let keys: Vec<(String, String)> = self.presented.lock().await.keys().cloned().collect();
        for (process, name) in keys {
            if let Err(e) = self.release(&process, &name).await {
                warn!(process = %process, golden = %name, error = %e, "golden not released");
            }
        }
    }

    pub async fn list(&self) -> Vec<Presented> {
        let mut v: Vec<Presented> = self.presented.lock().await.values().cloned().collect();
        v.sort_by(|a, b| (&a.process, &a.name).cmp(&(&b.process, &b.name)));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::{Goldens, Host};
    use crate::config::{GoldenMount, GoldensConfig};
    use axum::extract::{Path as AxPath, RawQuery, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    /// A stand-in for the stormblock engine: two goldens, ublk attaches, and
    /// a log of what it was asked.
    #[derive(Default)]
    struct Engine {
        log: Mutex<Vec<String>>,
        /// Refuse this many attaches with a 409 first.
        refuse: AtomicU32,
    }

    async fn serve(engine: Arc<Engine>) -> String {
        async fn list(State(e): State<Arc<Engine>>, q: RawQuery, h: HeaderMap) -> Json<serde_json::Value> {
            e.log.lock().unwrap().push(format!("GET volumes?{} auth={:?}", q.0.unwrap_or_default(), h.get("authorization")));
            Json(serde_json::json!({ "items": [
                { "id": "v-a", "name": "golden-a" },
                { "id": "v-b", "name": "golden-b" },
            ], "count": 2, "generation": 1 }))
        }
        async fn attach(
            State(e): State<Arc<Engine>>,
            AxPath(id): AxPath<String>,
            Json(body): Json<serde_json::Value>,
        ) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
            e.log.lock().unwrap().push(format!("POST {id} {body}"));
            if e.refuse.load(Ordering::SeqCst) > 0 {
                e.refuse.fetch_sub(1, Ordering::SeqCst);
                return Err((StatusCode::CONFLICT, "busy".into()));
            }
            let dev = if id == "v-a" { "/dev/ublkb1" } else { "/dev/ublkb2" };
            Ok(Json(serde_json::json!({ "transport": "ublk", "device_hint": dev })))
        }
        async fn detach(State(e): State<Arc<Engine>>, AxPath(id): AxPath<String>, q: RawQuery) -> StatusCode {
            e.log.lock().unwrap().push(format!("DELETE {id}?{}", q.0.unwrap_or_default()));
            StatusCode::OK
        }
        let app = Router::new()
            .route("/api/v1/volumes", get(list))
            .route("/api/v1/volumes/{id}/attach", post(attach).delete(detach))
            .with_state(engine);
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        url
    }

    /// Records what would be done to the filesystem.
    #[derive(Default)]
    struct FakeHost {
        ops: Mutex<Vec<String>>,
        fail_mount: std::sync::atomic::AtomicBool,
    }

    impl Host for FakeHost {
        fn devno(&self, device: &Path) -> std::io::Result<u64> {
            self.ops.lock().unwrap().push(format!("devno {}", device.display()));
            Ok(259 * 256 + 1)
        }
        fn make_node(&self, path: &Path, devno: u64, mode: u32, owner: Option<(u32, u32)>) -> std::io::Result<()> {
            self.ops.lock().unwrap().push(format!("mknod {} {devno} {mode:o} {owner:?}", path.display()));
            Ok(())
        }
        fn mkdir_p(&self, path: &Path) -> std::io::Result<()> {
            self.ops.lock().unwrap().push(format!("mkdir {}", path.display()));
            Ok(())
        }
        fn mount_ro(&self, device: &Path, path: &Path, fstype: &str) -> std::io::Result<()> {
            if self.fail_mount.load(Ordering::SeqCst) {
                return Err(std::io::Error::other("mount refused"));
            }
            self.ops.lock().unwrap().push(format!("mount ro {} {} {fstype}", device.display(), path.display()));
            Ok(())
        }
        fn unmount(&self, path: &Path) -> std::io::Result<()> {
            self.ops.lock().unwrap().push(format!("umount {}", path.display()));
            Ok(())
        }
        fn remove(&self, path: &Path) -> std::io::Result<()> {
            self.ops.lock().unwrap().push(format!("rm {}", path.display()));
            Ok(())
        }
    }

    fn golden(toml_text: &str) -> GoldenMount {
        toml::from_str(toml_text).unwrap()
    }

    async fn setup(token: Option<&str>) -> (Arc<Engine>, Arc<FakeHost>, Goldens, PathBuf) {
        let engine = Arc::new(Engine::default());
        let url = serve(engine.clone()).await;
        let dir = std::env::temp_dir().join(format!("stormd-goldens-{}-{}", std::process::id(), rand_suffix()));
        std::fs::create_dir_all(&dir).unwrap();
        let token_file = dir.join("token");
        if let Some(t) = token {
            std::fs::write(&token_file, format!("{t}\n")).unwrap();
        }
        let host = Arc::new(FakeHost::default());
        let cfg = GoldensConfig { engine_url: url, token_file, dir: PathBuf::from("/goldens") };
        let g = Goldens::new(cfg, "minismbd".into(), host.clone());
        (engine, host, g, dir)
    }

    fn rand_suffix() -> u32 {
        static N: AtomicU32 = AtomicU32::new(0);
        N.fetch_add(1, Ordering::SeqCst)
    }

    fn log(e: &Engine) -> Vec<String> {
        e.log.lock().unwrap().clone()
    }

    fn ops(h: &FakeHost) -> Vec<String> {
        h.ops.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn a_filesystem_golden_is_attached_ro_and_mounted_read_only() {
        let (engine, host, g, dir) = setup(Some("sekret")).await;
        let fs = golden("name = \"nic-drivers\"\ngolden = \"golden-a\"\ncontent = \"filesystem\"\n");
        let p = g.present("minismbd", &fs).await.unwrap();
        assert_eq!((p.volume_id.as_str(), p.device.as_str(), p.path.as_str()), ("v-a", "/dev/ublkb1", "/goldens/nic-drivers"));

        let l = log(&engine);
        assert!(l[0].starts_with("GET volumes?kind=golden") && l[0].contains("Bearer sekret"), "{l:?}");
        let attach: serde_json::Value = serde_json::from_str(l[1].strip_prefix("POST v-a ").unwrap()).unwrap();
        assert_eq!(attach, serde_json::json!({ "mode": "ro", "transport": "ublk", "holder": "stormd/minismbd/minismbd/nic-drivers" }));
        assert_eq!(
            ops(&host),
            vec![
                "devno /dev/ublkb1".to_string(),
                format!("mknod /dev/ublkb1 {} 400 None", 259 * 256 + 1),
                "mkdir /goldens/nic-drivers".into(),
                "mount ro /dev/ublkb1 /goldens/nic-drivers ext4".into(),
            ]
        );
        assert_eq!(g.list().await.len(), 1);

        g.release("minismbd", "nic-drivers").await.unwrap();
        assert_eq!(ops(&host).last().unwrap(), "umount /goldens/nic-drivers");
        assert_eq!(log(&engine).last().unwrap(), "DELETE v-a?holder=stormd%2Fminismbd%2Fminismbd%2Fnic-drivers");
        assert!(g.list().await.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn an_image_golden_is_a_device_node_the_service_can_read() {
        let (engine, host, g, dir) = setup(None).await;
        let img = golden(
            "name = \"boot\"\nvolume_id = \"v-b\"\ncontent = \"image\"\npath = \"/srv/boot.img\"\n\
             owner = \"65532:65532\"\nmode = 0o440\nsize_bytes = 123456\n",
        );
        let p = g.present("minismbd", &img).await.unwrap();
        assert_eq!(p.size_bytes, Some(123456));
        let l = log(&engine);
        assert_eq!(l.len(), 1, "a volume_id needs no lookup: {l:?}");
        assert!(!l[0].contains("authorization"));
        assert_eq!(
            ops(&host),
            vec!["devno /dev/ublkb2".to_string(), format!("mknod /srv/boot.img {} 440 Some((65532, 65532))", 259 * 256 + 1)]
        );
        g.release("minismbd", "boot").await.unwrap();
        assert_eq!(ops(&host).last().unwrap(), "rm /srv/boot.img");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_failed_mount_releases_the_attach_and_an_unknown_name_is_an_error() {
        let (engine, host, g, dir) = setup(None).await;
        host.fail_mount.store(true, Ordering::SeqCst);
        let fs = golden("name = \"d\"\ngolden = \"golden-a\"\ncontent = \"filesystem\"\n");
        assert!(g.present("p", &fs).await.is_err());
        assert!(log(&engine).last().unwrap().starts_with("DELETE v-a"), "{:?}", log(&engine));
        assert!(g.list().await.is_empty());

        let missing = golden("name = \"x\"\ngolden = \"golden-zzz\"\ncontent = \"image\"\n");
        let e = g.present("p", &missing).await.unwrap_err().to_string();
        assert!(e.contains("no golden named golden-zzz"), "{e}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn present_all_retries_until_the_engine_attaches_and_gives_up_on_shutdown() {
        let (engine, _host, g, dir) = setup(None).await;
        engine.refuse.store(1, Ordering::SeqCst);
        let fs = golden("name = \"d\"\ngolden = \"golden-a\"\ncontent = \"filesystem\"\n");
        let t = std::time::Instant::now();
        assert!(g.present_all("p", std::slice::from_ref(&fs), || false).await);
        assert!(t.elapsed() >= std::time::Duration::from_secs(2), "no retry delay");
        assert_eq!(g.list().await.len(), 1);

        engine.refuse.store(100, Ordering::SeqCst);
        let other = golden("name = \"e\"\ngolden = \"golden-b\"\ncontent = \"image\"\n");
        let stop = std::sync::atomic::AtomicBool::new(false);
        let gave_up = tokio::join!(g.present_all("p", std::slice::from_ref(&other), || stop.load(Ordering::SeqCst)), async {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            stop.store(true, Ordering::SeqCst);
        })
        .0;
        assert!(!gave_up, "present_all should give up once told to stop");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A swap stops the process, releases the old golden, presents the new
    /// one and starts the process again.
    #[tokio::test]
    async fn a_swap_restarts_the_process_on_the_new_golden() {
        use crate::supervisor::{ProcessState, Supervisor};
        let (engine, host, g, dir) = setup(None).await;
        let cfg: crate::config::Config = toml::from_str(&format!(
            "[general]\nname = \"t\"\nlog_dir = \"{}\"\n[stormlog.mcast]\ngroup = \"off\"\n",
            dir.display()
        ))
        .unwrap();
        let mut log_cfg = cfg.stormlog.clone();
        log_cfg.file.log_dir = dir.clone();
        let bus = Arc::new(crate::events::EventBus::new(cfg.events.clone(), "t".into()));
        let slog = Arc::new(stormlog::StormLog::new(log_cfg, "t"));
        let sup = Arc::new(Supervisor::new(slog, bus));
        let g = Arc::new(g);
        sup.set_goldens(g.clone());
        let proc: crate::config::ProcessConfig = toml::from_str(
            "name = \"smb\"\ncommand = \"/bin/sleep\"\nargs = [\"30\"]\n\
             [[golden]]\nname = \"boot\"\ngolden = \"golden-a\"\ncontent = \"filesystem\"\n",
        )
        .unwrap();
        sup.start_all(&[proc]).await.unwrap();
        let before = sup.get_status("smb").await.unwrap();
        assert_eq!(before.state, ProcessState::Running);
        assert_eq!(g.list().await[0].volume_id, "v-a", "presented before the first start");

        let p = sup.swap_golden("smb", "boot", Some("golden-b".into()), None).await.unwrap();
        let after = sup.get_status("smb").await.unwrap();
        let l = log(&engine);
        let o = ops(&host);
        sup.stop_all().await;
        g.release_all().await;
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!((p.volume_id.as_str(), p.device.as_str()), ("v-b", "/dev/ublkb2"));
        assert_eq!(after.state, ProcessState::Running);
        assert_ne!(after.pid, before.pid, "the process was not restarted");
        let del = l.iter().position(|x| x.starts_with("DELETE v-a")).expect("old golden not detached");
        let att = l.iter().position(|x| x.starts_with("POST v-b")).expect("new golden not attached");
        assert!(del < att, "attached the new one before releasing the old: {l:?}");
        let um = o.iter().position(|x| x == "umount /goldens/boot").unwrap();
        let mo = o.iter().rposition(|x| x == "mount ro /dev/ublkb2 /goldens/boot ext4").unwrap();
        assert!(um < mo, "{o:?}");
    }

    fn status_of(e: &anyhow::Error) -> u16 {
        e.downcast_ref::<super::Refused>().map(|r| r.status).unwrap_or(500)
    }

    /// stormd#73: a golden the config does not name is added to a running
    /// process without stopping it, re-asserted with no effect, replaced in
    /// place, and removed again — the process's pid unchanged throughout.
    #[tokio::test]
    async fn an_added_golden_comes_and_goes_with_the_process_left_running() {
        use crate::supervisor::{ProcessState, Supervisor};
        use super::Added;
        let (engine, host, g, dir) = setup(None).await;
        let cfg: crate::config::Config = toml::from_str(&format!(
            "[general]\nname = \"t\"\nlog_dir = \"{}\"\n[stormlog.mcast]\ngroup = \"off\"\n",
            dir.display()
        ))
        .unwrap();
        let mut log_cfg = cfg.stormlog.clone();
        log_cfg.file.log_dir = dir.clone();
        let bus = Arc::new(crate::events::EventBus::new(cfg.events.clone(), "t".into()));
        let slog = Arc::new(stormlog::StormLog::new(log_cfg, "t"));
        let sup = Arc::new(Supervisor::new(slog, bus));
        let g = Arc::new(g);
        sup.set_goldens(g.clone());
        // minismbd's shape: one configured filesystem golden, the boot
        // helpers added at runtime as images.
        let proc: crate::config::ProcessConfig = toml::from_str(
            "name = \"smb\"\ncommand = \"/bin/sleep\"\nargs = [\"30\"]\n\
             [[golden]]\nname = \"base\"\ngolden = \"golden-a\"\ncontent = \"filesystem\"\n",
        )
        .unwrap();
        sup.start_all(&[proc]).await.unwrap();
        let pid = sup.get_status("smb").await.unwrap().pid;
        assert!(pid.is_some());
        let body = serde_json::json!({ "golden": "golden-b", "content": "image", "size_bytes": 4096, "owner": "65532:65532" });

        let (added, p) = sup.put_golden("smb", "ipxe", body.clone()).await.unwrap();
        assert_eq!(added, Added::Created);
        assert_eq!((p.path.as_str(), p.volume_id.as_str(), p.added), ("/goldens/ipxe", "v-b", true));
        assert!(ops(&host).iter().any(|o| o.starts_with("mknod /goldens/ipxe ") && o.ends_with("444 Some((65532, 65532))")), "{:?}", ops(&host));
        let listed = serde_json::to_value(g.list().await).unwrap();
        let flags: Vec<(String, bool)> = listed.as_array().unwrap().iter()
            .map(|v| (v["name"].as_str().unwrap().to_string(), v["added"].as_bool().unwrap())).collect();
        assert_eq!(flags, vec![("base".to_string(), false), ("ipxe".to_string(), true)]);

        // stormipmi re-asserts every pass: nothing is done.
        let n = log(&engine).len();
        let (again, _) = sup.put_golden("smb", "ipxe", body.clone()).await.unwrap();
        assert_eq!(again, Added::Unchanged);
        assert_eq!(log(&engine).len(), n, "a re-add of the same spec reached the engine");

        // A new release under the same name: replaced, no stop/start.
        let (re, p2) = sup.put_golden("smb", "ipxe", serde_json::json!({ "volume_id": "v-a", "content": "image" })).await.unwrap();
        assert_eq!((re, p2.volume_id.as_str()), (Added::Replaced, "v-a"));
        assert!(log(&engine).iter().any(|l| l.starts_with("DELETE v-b")));

        // Refusals.
        let e = sup.put_golden("smb", "bad", serde_json::json!({ "golden": "golden-b" })).await.unwrap_err();
        assert_eq!(status_of(&e), 400, "{e}");
        let e = sup.put_golden("smb", "two", serde_json::json!({ "golden": "golden-b", "volume_id": "v-b", "content": "image" })).await.unwrap_err();
        assert_eq!(status_of(&e), 400, "{e}");
        let e = sup.put_golden("smb", "clash", serde_json::json!({ "golden": "golden-b", "content": "image", "path": "/goldens/ipxe" })).await.unwrap_err();
        assert_eq!(status_of(&e), 409, "{e}");
        let e = sup.put_golden("smb", "clash", serde_json::json!({ "golden": "golden-b", "content": "image", "path": "/goldens/base" })).await.unwrap_err();
        assert_eq!(status_of(&e), 409, "{e}");
        let e = sup.put_golden("nope", "x", body.clone()).await.unwrap_err();
        assert_eq!(status_of(&e), 404, "{e}");
        let e = sup.remove_golden("smb", "base").await.unwrap_err();
        assert_eq!(status_of(&e), 409, "{e}");
        let e = sup.remove_golden("smb", "never").await.unwrap_err();
        assert_eq!(status_of(&e), 404, "{e}");

        sup.remove_golden("smb", "ipxe").await.unwrap();
        assert_eq!(ops(&host).last().unwrap(), "rm /goldens/ipxe");
        assert_eq!(log(&engine).last().unwrap(), "DELETE v-a?holder=stormd%2Fminismbd%2Fsmb%2Fipxe");
        let names: Vec<String> = g.list().await.into_iter().map(|p| p.name).collect();

        let after = sup.get_status("smb").await.unwrap();
        sup.stop_all().await;
        g.release_all().await;
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(names, vec!["base".to_string()]);
        assert_eq!(after.state, ProcessState::Running);
        assert_eq!(after.pid, pid, "the process was restarted by an add or a remove");
    }

    /// A failed add leaves nothing attached and nothing listed; a failed
    /// replace puts the old golden back.
    #[tokio::test]
    async fn a_failed_add_leaves_nothing_attached() {
        let (engine, host, g, dir) = setup(None).await;
        host.fail_mount.store(true, Ordering::SeqCst);
        let fs = golden("name = \"d\"\ngolden = \"golden-a\"\ncontent = \"filesystem\"\n");
        assert!(g.add("p", fs.clone(), &[]).await.is_err());
        let l = log(&engine);
        assert!(l.iter().any(|x| x.starts_with("POST v-a")) && l.last().unwrap().starts_with("DELETE v-a"), "{l:?}");
        assert!(g.list().await.is_empty());

        let missing = golden("name = \"x\"\ngolden = \"golden-zzz\"\ncontent = \"image\"\n");
        assert!(g.add("p", missing, &[]).await.is_err());
        assert!(g.list().await.is_empty());

        host.fail_mount.store(false, Ordering::SeqCst);
        g.add("p", fs.clone(), &[]).await.unwrap();
        // A replace whose new golden cannot be presented puts the old back.
        let newer = GoldenMount { golden: Some("golden-zzz".into()), ..fs };
        assert!(g.add("p", newer, &[]).await.is_err());
        let listed = g.list().await;
        assert_eq!((listed.len(), listed[0].volume_id.as_str(), listed[0].added), (1, "v-a", true));
        assert_eq!(ops(&host).last().unwrap(), "mount ro /dev/ublkb1 /goldens/d ext4");
        let _ = std::fs::remove_dir_all(dir);
    }
}
