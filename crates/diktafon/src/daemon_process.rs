//! The diktafond process itself: finding it from its pidfile, confirming a pid
//! is really ours, and stopping it. Pids get recycled, so signalling one is
//! only safe after the name check — keeping the check and the signal in one
//! module is what stops a caller from doing the second without the first.
//! Both the transport (retiring a version-mismatched daemon) and the menu bar
//! (Quit) need this.

use std::path::Path;

/// The pid diktafond recorded for the daemon serving `socket`, if any.
pub fn pid_for(socket: &Path) -> Option<u32> {
    read_pid(&diktafon_protocol::pid_path_for(socket))
}

/// The pid for the daemon on the default socket.
pub fn pid() -> Option<u32> {
    read_pid(&diktafon_protocol::pid_path())
}

fn read_pid(path: &Path) -> Option<u32> {
    let pid = std::fs::read_to_string(path).ok()?.trim().parse().ok()?;
    (pid != 0).then_some(pid)
}

/// Cheap liveness check with no subprocess, for paths that run while the menu
/// opens. A recycled pid can fool it, which only mislabels a dead daemon as
/// running until the next dictation; anything destructive uses
/// [`is_diktafond`] instead.
pub fn is_alive(pid: u32) -> bool {
    pid != 0 && unsafe { libc::kill(pid as i32, 0) } == 0
}

/// Strict check for destructive paths: the pid must still name a diktafond.
pub fn is_diktafond(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    std::process::Command::new("ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .output()
        .map(|out| names_a_daemon(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or(false)
}

/// `ps -o comm=` prints the executable path, so the daemon is matched by its
/// file name. The client's own path ends in "diktafon" and must not match.
fn names_a_daemon(comm: &str) -> bool {
    comm.trim().ends_with("diktafond")
}

/// Why a daemon could not be stopped, so callers can tell the user which
/// happened: they mean different things for what to do next.
#[derive(Debug, PartialEq)]
pub enum StopError {
    /// The pid names something else now; pids get recycled.
    NotOurs,
    /// It is ours, but the signal did not land (it exited first, or we lack
    /// permission).
    SignalFailed,
}

/// SIGTERM the pid, but only once it still names a diktafond.
pub fn stop(pid: u32) -> Result<(), StopError> {
    if !is_diktafond(pid) {
        return Err(StopError::NotOurs);
    }
    // Signalling directly rather than shelling out: `kill` exits non-zero for
    // a process that just died, which is indistinguishable from a spawn
    // failure in an exit status.
    match unsafe { libc::kill(pid as i32, libc::SIGTERM) } {
        0 => Ok(()),
        _ => Err(StopError::SignalFailed),
    }
}

/// Stop the daemon on the default socket; quiet no-op when none is running.
pub fn stop_running() {
    if let Some(pid) = pid() {
        let _ = stop(pid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn liveness_rejects_zero_and_accepts_this_process() {
        assert!(!is_alive(0));
        assert!(is_alive(std::process::id()));
    }

    #[test]
    fn only_the_daemon_binary_matches_its_name() {
        assert!(names_a_daemon(
            "/Applications/diktafon.app/Contents/MacOS/diktafond"
        ));
        assert!(names_a_daemon("  /usr/local/bin/diktafond\n"));
        // The client's own binary is one letter short of matching.
        assert!(!names_a_daemon(
            "/Applications/diktafon.app/Contents/MacOS/diktafon"
        ));
        assert!(!names_a_daemon("/bin/zsh"));
        assert!(!names_a_daemon(""));
    }

    #[test]
    fn this_process_is_never_signalled() {
        assert!(!is_diktafond(0));
        assert_eq!(stop(std::process::id()), Err(StopError::NotOurs));
    }

    #[test]
    fn a_missing_or_empty_pidfile_yields_nothing() {
        let dir = std::env::temp_dir().join(format!("dkt-pid-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        assert_eq!(read_pid(&dir.join("absent.pid")), None);
        let empty = dir.join("empty.pid");
        std::fs::write(&empty, "").unwrap();
        assert_eq!(read_pid(&empty), None);
        let zero = dir.join("zero.pid");
        std::fs::write(&zero, "0\n").unwrap();
        assert_eq!(read_pid(&zero), None);
        let real = dir.join("real.pid");
        std::fs::write(&real, "  4321\n").unwrap();
        assert_eq!(read_pid(&real), Some(4321));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
