//! The volumes stormd's own container runs on (stormd#75, option B).
//!
//! `GET /api/v1/volumes` says which stormblock volumes this container is
//! actually using: its root clone, its `/d/*` data and logs volumes and the
//! goldens it attached. stormd creates none of them (release staging does,
//! stormblock#670); it only knows where they are mounted. So the report is
//! built from what the container sees, then named by the node's engine:
//!
//! 1. `/proc/self/mountinfo` gives every mount and its device number;
//! 2. a mount on a ublk device (`/sys/dev/block/<maj:min>/uevent` says
//!    `DEVNAME=ublkbN`, else the mount source is `/dev/ublkbN`) is a volume;
//! 3. the engine's `GET /api/v1/volumes?in_use=true` lists each attachment's
//!    ublk device, which gives the volume id, name, kind and owner.
//!
//! Each volume's `state` is `attached` (the engine serves that device as
//! that volume), `missing` (mounted from a device the engine no longer
//! serves, or a golden a process declares that is not presented) or
//! `unverified` (the engine did not answer). rustkube-node puts the report on
//! the service's mirror pod. Mounts on anything but a ublk device (tmpfs,
//! proc, a host bind of a non-volume path) are not volumes and are left out.

use crate::config::{GoldenContent, GoldenMount};
use crate::goldens::Presented;
use serde::Serialize;
use std::collections::BTreeMap;

/// One line of `/proc/self/mountinfo`, the fields the report needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountEntry {
    pub major: u32,
    pub minor: u32,
    pub mount_point: String,
    pub fstype: String,
    pub source: String,
}

/// `\040`-style octal escapes, as the kernel writes spaces and the like.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            let v = (b[i + 1] - b'0') as u32 * 64 + (b[i + 2] - b'0') as u32 * 8 + (b[i + 3] - b'0') as u32;
            out.push(v as u8);
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parse mountinfo (proc(5)): `id parent maj:min root mountpoint opts
/// [optional…] - fstype source superopts`. Malformed lines are skipped.
pub fn parse_mountinfo(text: &str) -> Vec<MountEntry> {
    text.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split(' ').collect();
            let sep = f.iter().position(|x| *x == "-")?;
            if sep < 6 || f.len() < sep + 3 {
                return None;
            }
            let (maj, min) = f[2].split_once(':')?;
            Some(MountEntry {
                major: maj.parse().ok()?,
                minor: min.parse().ok()?,
                mount_point: unescape(f[4]),
                fstype: f[sep + 1].to_string(),
                source: unescape(f[sep + 2]),
            })
        })
        .collect()
}

/// `/dev/ublkbN` with only digits after it.
fn is_ublk(dev: &str) -> bool {
    dev.strip_prefix("/dev/ublkb").is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
}

/// The ublk device a mount is on, if it is on one. `devname` reads the
/// kernel's name for a device number (sysfs); it wins over the mount
/// source, which is only what the mounter wrote.
pub fn ublk_device(m: &MountEntry, devname: &dyn Fn(u32, u32) -> Option<String>) -> Option<String> {
    if let Some(name) = devname(m.major, m.minor) {
        let dev = format!("/dev/{name}");
        return is_ublk(&dev).then_some(dev);
    }
    is_ublk(&m.source).then(|| m.source.clone())
}

/// The kernel's name for a block device number, from
/// `/sys/dev/block/<maj>:<min>/uevent` (`DEVNAME=ublkb3`).
pub fn sysfs_devname(major: u32, minor: u32) -> Option<String> {
    let text = std::fs::read_to_string(format!("/sys/dev/block/{major}:{minor}/uevent")).ok()?;
    text.lines().find_map(|l| l.strip_prefix("DEVNAME=")).map(|s| s.trim().to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// The engine serves this device as this volume.
    Attached,
    /// Mounted from a device the engine does not serve, or declared and not
    /// presented: the service is running without (or on a stale) volume.
    Missing,
    /// The engine did not answer; the device is mounted, its volume unknown.
    Unverified,
}

/// A golden, by the process that declares it and its name there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GoldenRef {
    pub process: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Volume {
    /// The engine's volume id; absent when it is not known.
    pub volume_id: Option<String>,
    /// The engine's name for it (`cadvisor-logs`, `golden-…`).
    pub name: Option<String>,
    /// The engine's kind (`volume`, `golden`, …).
    pub kind: Option<String>,
    /// `root` (mounted at `/`), `golden`, `data` / `logs` (the engine name's
    /// suffix), else `volume`.
    pub role: String,
    pub device: Option<String>,
    pub fstype: Option<String>,
    /// Where it is mounted in this container (several for bind mounts; none
    /// for an image golden, which is a device node).
    pub mounts: Vec<String>,
    pub state: State,
    /// Why it is not `attached`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The engine's `owner` for it, as the engine has it.
    pub owner: Option<serde_json::Value>,
    /// The goldens this is, when it is one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub goldens: Vec<GoldenRef>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    /// This container's name (`[general] name`).
    pub container: String,
    /// Why the engine could not name the volumes, when it could not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine_error: Option<String>,
    pub items: Vec<Volume>,
}

/// The engine volume served as `device` over ublk, from an `?in_use=true`
/// listing's items.
fn engine_volume<'a>(engine: &'a [serde_json::Value], device: &str) -> Option<&'a serde_json::Value> {
    engine.iter().find(|v| {
        v["attachments"]
            .as_array()
            .is_some_and(|a| a.iter().any(|x| x["transport"] == "ublk" && x["device"] == device))
    })
}

fn role_of(name: Option<&str>, mounts: &[String], golden: bool) -> String {
    if mounts.iter().any(|m| m == "/") {
        return "root".into();
    }
    if golden {
        return "golden".into();
    }
    match name {
        Some(n) if n.ends_with("-data") => "data".into(),
        Some(n) if n.ends_with("-logs") => "logs".into(),
        _ => "volume".into(),
    }
}

/// Build the report. `engine` is the engine's in-use listing (`Err`: why it
/// could not be had); `presented` the goldens stormd attached; `declared`
/// every golden a process's config names.
pub fn report(
    container: &str,
    mounts: &[MountEntry],
    devname: &dyn Fn(u32, u32) -> Option<String>,
    engine: Result<Vec<serde_json::Value>, String>,
    presented: &[Presented],
    declared: &[(String, GoldenMount)],
) -> Report {
    // Device -> (fstype, mount points), in mountinfo order.
    let mut devices: BTreeMap<String, (Option<String>, Vec<String>)> = BTreeMap::new();
    for m in mounts {
        if let Some(dev) = ublk_device(m, devname) {
            let e = devices.entry(dev).or_insert((Some(m.fstype.clone()), Vec::new()));
            if !e.1.contains(&m.mount_point) {
                e.1.push(m.mount_point.clone());
            }
        }
    }
    // An image golden is a device node, not a mount: it is used all the same.
    for p in presented {
        let e = devices.entry(p.device.clone()).or_insert((None, Vec::new()));
        if p.content == GoldenContent::Filesystem && !e.1.contains(&p.path) {
            e.1.push(p.path.clone());
        }
    }

    let (engine, engine_error) = match engine {
        Ok(v) => (Some(v), None),
        Err(e) => (None, Some(e)),
    };
    let mut items = Vec::new();
    for (device, (fstype, mut mount_points)) in devices {
        mount_points.sort();
        let goldens: Vec<GoldenRef> = presented
            .iter()
            .filter(|p| p.device == device)
            .map(|p| GoldenRef { process: p.process.clone(), name: p.name.clone() })
            .collect();
        let ours = presented.iter().find(|p| p.device == device);
        let (volume_id, name, kind, owner, state, reason) = match &engine {
            Some(list) => match engine_volume(list, &device) {
                Some(v) => (
                    v["id"].as_str().map(String::from),
                    v["name"].as_str().map(String::from),
                    v["kind"].as_str().map(String::from),
                    v.get("owner").filter(|o| !o.is_null()).cloned(),
                    State::Attached,
                    None,
                ),
                None => (
                    ours.map(|p| p.volume_id.clone()),
                    ours.and_then(|p| p.golden.clone()),
                    None,
                    None,
                    State::Missing,
                    Some(format!("the engine serves no volume as {device}")),
                ),
            },
            None => (
                ours.map(|p| p.volume_id.clone()),
                ours.and_then(|p| p.golden.clone()),
                None,
                None,
                State::Unverified,
                Some("the engine did not answer".to_string()),
            ),
        };
        let role = role_of(name.as_deref(), &mount_points, !goldens.is_empty());
        items.push(Volume {
            volume_id,
            name,
            kind,
            role,
            device: Some(device),
            fstype,
            mounts: mount_points,
            state,
            reason,
            owner,
            goldens,
        });
    }
    // Declared and not presented: the process is meant to have it.
    for (process, g) in declared {
        if presented.iter().any(|p| &p.process == process && p.name == g.name) {
            continue;
        }
        items.push(Volume {
            volume_id: g.volume_id.clone(),
            name: g.golden.clone(),
            kind: Some("golden".into()),
            role: "golden".into(),
            device: None,
            fstype: None,
            mounts: Vec::new(),
            state: State::Missing,
            reason: Some(format!("{process} declares golden {}; it is not presented", g.name)),
            owner: None,
            goldens: vec![GoldenRef { process: process.clone(), name: g.name.clone() }],
        });
    }
    // Root first, then by where it is mounted.
    items.sort_by(|a, b| (a.role != "root", a.mounts.first(), &a.device).cmp(&(b.role != "root", b.mounts.first(), &b.device)));
    Report { container: container.to_string(), engine_error, items }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOUNTINFO: &str = "\
1 0 259:3 / / rw,relatime - ext4 /dev/ublkb3 rw
2 1 0:20 / /proc rw,nosuid shared:2 - proc proc rw
3 1 0:21 / /dev rw - tmpfs tmpfs rw,mode=755
4 1 259:7 / /var/log/stormd rw,relatime shared:9 master:4 - ext4 /dev/ublkb7 rw
5 1 259:7 /sub /also\\040here rw - ext4 /dev/ublkb7 rw
6 1 259:9 / /data rw - xfs /dev/ublkb9 rw
7 1 8:1 / /host rw - ext4 /dev/sda1 rw
8 1 259:12 / /goldens/drivers ro - ext4 /dev/ublkb12 ro
bad line
";

    fn devname(maj: u32, min: u32) -> Option<String> {
        match (maj, min) {
            (259, 3) => Some("ublkb3".into()),
            (259, 7) => Some("ublkb7".into()),
            (259, 9) => Some("ublkb9".into()),
            (8, 1) => Some("sda1".into()),
            _ => None, // 259:12: no sysfs entry, the mount source decides
        }
    }

    fn engine() -> Vec<serde_json::Value> {
        serde_json::from_str(
            r#"[
            {"id":"r-1","name":"cadvisor","kind":"volume","in_use":true,
             "attachments":[{"transport":"ublk","device":"/dev/ublkb3","mounted_at":"/d/cadvisor"}],
             "owner":{"kind":"Service","namespace":"kube-system","name":"cadvisor"}},
            {"id":"l-1","name":"cadvisor-logs","kind":"volume","in_use":true,
             "attachments":[{"transport":"ublk","device":"/dev/ublkb7"}]},
            {"id":"g-1","name":"golden-drivers-1","kind":"golden","in_use":true,
             "attachments":[{"transport":"ublk","device":"/dev/ublkb12"}]},
            {"id":"x-1","name":"other-data","kind":"volume","in_use":true,
             "attachments":[{"transport":"nvme-tcp","device":"/dev/ublkb9"}]}
        ]"#,
        )
        .unwrap()
    }

    fn golden(name: &str, golden: &str) -> GoldenMount {
        GoldenMount {
            name: name.into(),
            golden: Some(golden.into()),
            volume_id: None,
            content: GoldenContent::Filesystem,
            path: None,
            fstype: "ext4".into(),
            owner: None,
            mode: 0o444,
            size_bytes: None,
        }
    }

    fn presented() -> Vec<Presented> {
        vec![Presented {
            process: "svc".into(),
            name: "drivers".into(),
            golden: Some("golden-drivers-1".into()),
            volume_id: "g-1".into(),
            content: GoldenContent::Filesystem,
            device: "/dev/ublkb12".into(),
            path: "/goldens/drivers".into(),
            size_bytes: None,
            added: false,
            spec: golden("drivers", "golden-drivers-1"),
        }]
    }

    #[test]
    fn mountinfo_lines_parse_with_optional_fields_and_escapes() {
        let m = parse_mountinfo(MOUNTINFO);
        assert_eq!(m.len(), 8, "the bad line is skipped");
        assert_eq!(
            m[3],
            MountEntry { major: 259, minor: 7, mount_point: "/var/log/stormd".into(), fstype: "ext4".into(), source: "/dev/ublkb7".into() }
        );
        assert_eq!(m[4].mount_point, "/also here");
    }

    #[test]
    fn a_ublk_device_comes_from_sysfs_then_the_mount_source() {
        let m = parse_mountinfo(MOUNTINFO);
        assert_eq!(ublk_device(&m[0], &devname).as_deref(), Some("/dev/ublkb3"));
        assert_eq!(ublk_device(&m[1], &devname), None, "proc");
        assert_eq!(ublk_device(&m[6], &devname), None, "sda1 is not a volume");
        assert_eq!(ublk_device(&m[7], &devname).as_deref(), Some("/dev/ublkb12"), "source fallback");
        let lying = MountEntry { major: 8, minor: 1, mount_point: "/x".into(), fstype: "ext4".into(), source: "/dev/ublkb1".into() };
        assert_eq!(ublk_device(&lying, &devname), None, "sysfs wins over the source");
        assert!(!is_ublk("/dev/ublkb") && !is_ublk("/dev/ublkb1p1") && !is_ublk("/dev/ublkc1"));
    }

    #[test]
    fn the_report_names_each_volume_by_the_engine() {
        let declared = vec![("svc".to_string(), golden("drivers", "golden-drivers-1")), ("svc".to_string(), golden("boot", "golden-boot-2"))];
        let r = report("cadvisor", &parse_mountinfo(MOUNTINFO), &devname, Ok(engine()), &presented(), &declared);
        assert_eq!(r.container, "cadvisor");
        assert!(r.engine_error.is_none());
        let by: Vec<(&str, Option<&str>, Option<&str>, State)> =
            r.items.iter().map(|v| (v.role.as_str(), v.name.as_deref(), v.device.as_deref(), v.state)).collect();
        assert_eq!(
            by,
            vec![
                ("root", Some("cadvisor"), Some("/dev/ublkb3"), State::Attached),
                ("golden", Some("golden-boot-2"), None, State::Missing),
                ("logs", Some("cadvisor-logs"), Some("/dev/ublkb7"), State::Attached),
                ("volume", None, Some("/dev/ublkb9"), State::Missing),
                ("golden", Some("golden-drivers-1"), Some("/dev/ublkb12"), State::Attached),
            ]
        );
        let root = &r.items[0];
        assert_eq!(root.volume_id.as_deref(), Some("r-1"));
        assert_eq!(root.owner.as_ref().unwrap()["kind"], "Service");
        let logs = &r.items[2];
        assert_eq!(logs.mounts, vec!["/also here".to_string(), "/var/log/stormd".to_string()], "one volume, both mounts");
        assert_eq!(logs.owner, None, "no owner set on the engine");
        let gone = &r.items[3];
        assert_eq!(gone.reason.as_deref(), Some("the engine serves no volume as /dev/ublkb9"), "an nvme-tcp row is not this device");
        let boot = &r.items[1];
        assert_eq!(boot.goldens, vec![GoldenRef { process: "svc".into(), name: "boot".into() }]);
        assert_eq!(boot.reason.as_deref(), Some("svc declares golden boot; it is not presented"));
        assert_eq!(r.items[4].goldens, vec![GoldenRef { process: "svc".into(), name: "drivers".into() }]);
        assert_eq!(r.items[4].volume_id.as_deref(), Some("g-1"));
    }

    #[test]
    fn without_the_engine_every_device_is_unverified() {
        let r = report("c", &parse_mountinfo(MOUNTINFO), &devname, Err("refused".into()), &presented(), &[]);
        assert_eq!(r.engine_error.as_deref(), Some("refused"));
        assert_eq!(r.items.len(), 4);
        assert!(r.items.iter().all(|v| v.state == State::Unverified));
        let g = r.items.iter().find(|v| v.device.as_deref() == Some("/dev/ublkb12")).unwrap();
        assert_eq!((g.volume_id.as_deref(), g.role.as_str()), (Some("g-1"), "golden"), "what stormd knows of its own golden");
    }

    #[test]
    fn an_image_golden_is_reported_without_a_mount() {
        let mut p = presented();
        p[0].content = GoldenContent::Image;
        p[0].device = "/dev/ublkb20".into();
        let eng: Vec<serde_json::Value> =
            serde_json::from_str(r#"[{"id":"g-1","name":"golden-drivers-1","kind":"golden","attachments":[{"transport":"ublk","device":"/dev/ublkb20"}]}]"#).unwrap();
        let r = report("c", &[], &devname, Ok(eng), &p, &[]);
        assert_eq!(r.items.len(), 1);
        assert_eq!((r.items[0].state, r.items[0].mounts.len(), r.items[0].fstype.clone()), (State::Attached, 0, None));
    }
}
