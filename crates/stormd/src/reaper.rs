//! PID 1 zombie reaper — required when stormd runs as init in a container.
//!
//! **The reaper stole supervised children's exit status** (stormd#61). It
//! ran `waitpid(-1)` every second, which reaps *any* child — including one
//! stormd itself is waiting on. When the reaper won the race, `child.wait()`
//! failed with ECHILD and the run was recorded as `exited with error
//! code=None`: a one-shot that had finished cleanly counted as failed.
//!
//! Now the reaper reaps only zombies nobody else has: it scans `/proc` for
//! zombie children of stormd and reaps one (by its pid) only once it has
//! stayed unreaped for [`GRACE`]. stormd's own waiters (supervisor, cron,
//! probes, shell) reap their children within milliseconds of the exit;
//! what is still a zombie seconds later is an orphan re-parented to PID 1,
//! or a child whose handle was dropped unwaited.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long a zombie must stay unreaped before the reaper takes it.
pub const GRACE: Duration = Duration::from_secs(5);

/// How often the reaper looks.
pub const INTERVAL: Duration = Duration::from_secs(1);

/// `(pid, state, ppid)` from the contents of `/proc/<pid>/stat`.
///
/// The command name sits in parentheses and may itself hold `)` or spaces,
/// so the fields after it are found from the *last* `)`.
pub fn parse_stat(stat: &str) -> Option<(i32, char, i32)> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let pid = stat[..open].trim().parse().ok()?;
    let mut rest = stat.get(close + 1..)?.split_whitespace();
    let state = rest.next()?.chars().next()?;
    let ppid = rest.next()?.parse().ok()?;
    Some((pid, state, ppid))
}

/// The zombie children of `parent`, from `/proc`. `None` if `/proc` cannot
/// be read at all.
pub fn zombie_children(parent: i32) -> Option<Vec<i32>> {
    let dir = std::fs::read_dir("/proc").ok()?;
    let mut out = Vec::new();
    for entry in dir.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|n| n.parse::<i32>().ok()) else {
            continue;
        };
        // A process gone between readdir and read is no zombie of ours.
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };
        if let Some((_, 'Z', ppid)) = parse_stat(&stat) {
            if ppid == parent {
                out.push(pid);
            }
        }
    }
    Some(out)
}

/// When each zombie was first seen; says which are due to be reaped.
#[derive(Default)]
pub struct Tracker {
    first_seen: HashMap<i32, Instant>,
}

impl Tracker {
    /// Record this scan's zombies; return those seen for at least `grace`.
    /// A pid missing from the scan was reaped by its owner and is forgotten
    /// (so a later process given the same pid starts its own clock).
    pub fn due(&mut self, zombies: &[i32], now: Instant, grace: Duration) -> Vec<i32> {
        self.first_seen.retain(|pid, _| zombies.contains(pid));
        let mut due = Vec::new();
        for &pid in zombies {
            let seen = *self.first_seen.entry(pid).or_insert(now);
            if now.duration_since(seen) >= grace {
                due.push(pid);
            }
        }
        for pid in &due {
            self.first_seen.remove(pid);
        }
        due
    }
}

/// Reap `pid` if it is still a zombie child; true if it was reaped here.
pub fn reap(pid: i32) -> bool {
    use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
    use nix::unistd::Pid;
    match waitpid(Pid::from_raw(pid), Some(WaitPidFlag::WNOHANG)) {
        Ok(WaitStatus::StillAlive) | Err(_) => false,
        Ok(status) => {
            tracing::debug!(?status, "reaped orphaned zombie");
            true
        }
    }
}

/// The reaper loop. Runs for the life of stormd.
pub async fn run() {
    let me = std::process::id() as i32;
    let mut tracker = Tracker::default();
    let mut warned = false;
    loop {
        tokio::time::sleep(INTERVAL).await;
        match zombie_children(me) {
            Some(zombies) => {
                for pid in tracker.due(&zombies, Instant::now(), GRACE) {
                    reap(pid);
                }
            }
            None => {
                // No /proc: fall back to reaping any child. This can take a
                // supervised child's exit status (stormd#61), but zombies
                // piling up under PID 1 is worse.
                if !warned {
                    tracing::warn!("/proc is not readable — reaping any exited child; a supervised process's exit code may be lost (stormd#61)");
                    warned = true;
                }
                reap_any();
            }
        }
    }
}

fn reap_any() {
    use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
    use nix::unistd::Pid;
    while let Ok(status) = waitpid(Pid::from_raw(-1), Some(WaitPidFlag::WNOHANG)) {
        if status == WaitStatus::StillAlive {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_with_odd_command_names() {
        assert_eq!(parse_stat("42 (sleep) S 1 42 42 0"), Some((42, 'S', 1)));
        assert_eq!(parse_stat("7 (a) b) Z 1 7 7"), Some((7, 'Z', 1)));
        assert_eq!(parse_stat("9 (two words) R 3 9"), Some((9, 'R', 3)));
        assert_eq!(parse_stat("garbage"), None);
    }

    #[test]
    fn a_zombie_is_due_only_after_the_grace() {
        let mut t = Tracker::default();
        let t0 = Instant::now();
        let g = Duration::from_secs(5);
        assert!(t.due(&[10, 11], t0, g).is_empty(), "first sighting");
        assert!(t.due(&[10, 11], t0 + Duration::from_secs(4), g).is_empty());
        // 11 was reaped by its owner meanwhile.
        assert_eq!(t.due(&[10], t0 + Duration::from_secs(5), g), vec![10]);
        // 11 again later is a new process: its own clock.
        assert!(t.due(&[11], t0 + Duration::from_secs(6), g).is_empty());
        assert_eq!(t.due(&[11], t0 + Duration::from_secs(11), g), vec![11]);
    }

    async fn wait_zombie(pid: i32) {
        for _ in 0..200 {
            if zombie_children(std::process::id() as i32).unwrap().contains(&pid) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{pid} never became a zombie");
    }

    /// The stormd#61 race: the child has exited and is a zombie, its owner
    /// has not reaped it yet, and the reaper runs. The owner still gets the
    /// exit code.
    #[tokio::test]
    async fn a_waited_child_keeps_its_exit_code() {
        let mut child = tokio::process::Command::new("/bin/sh")
            .args(["-c", "exit 3"])
            .spawn()
            .unwrap();
        let pid = child.id().unwrap() as i32;
        wait_zombie(pid).await;
        let mut t = Tracker::default();
        let zombies: Vec<i32> = zombie_children(std::process::id() as i32)
            .unwrap()
            .into_iter()
            .filter(|p| *p == pid)
            .collect();
        for p in t.due(&zombies, Instant::now(), GRACE) {
            reap(p);
        }
        let status = child.wait().await.expect("status not lost");
        assert_eq!(status.code(), Some(3));
    }

    /// A child nobody waits for is reaped once the grace has passed.
    #[tokio::test]
    async fn an_unwaited_zombie_is_reaped_after_the_grace() {
        let child = std::process::Command::new("/bin/sh").args(["-c", "exit 0"]).spawn().unwrap();
        let pid = child.id() as i32;
        drop(child); // std does not wait on drop: an orphan-like zombie
        wait_zombie(pid).await;
        let mut t = Tracker::default();
        let t0 = Instant::now();
        let ours = |now| {
            let z: Vec<i32> = zombie_children(std::process::id() as i32)
                .unwrap()
                .into_iter()
                .filter(|p| *p == pid)
                .collect();
            (z, now)
        };
        let (z, now) = ours(t0);
        assert!(t.due(&z, now, GRACE).is_empty());
        let (z, now) = ours(t0 + GRACE);
        assert_eq!(t.due(&z, now, GRACE), vec![pid]);
        assert!(reap(pid));
        assert!(!zombie_children(std::process::id() as i32).unwrap().contains(&pid));
    }
}
