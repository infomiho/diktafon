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
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .trim()
                .ends_with("diktafond")
        })
        .unwrap_or(false)
}

/// SIGTERM the pid if it still names a diktafond. Returns whether the signal
/// was sent, so callers can report a daemon they could not stop.
pub fn stop(pid: u32) -> bool {
    if !is_diktafond(pid) {
        return false;
    }
    std::process::Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .is_ok()
}

/// Stop the daemon on the default socket; quiet no-op when none is running.
pub fn stop_running() {
    if let Some(pid) = pid() {
        stop(pid);
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
    fn the_test_binary_is_not_a_daemon() {
        assert!(!is_diktafond(0));
        // Guards the suffix check: "diktafon" must not match "diktafond".
        assert!(!is_diktafond(std::process::id()));
        assert!(!stop(std::process::id()));
    }

    #[test]
    fn a_missing_or_empty_pidfile_yields_nothing() {
        let dir = std::env::temp_dir().join(format!("dkt-pid-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        assert_eq!(read_pid(&dir.join("absent.pid")), None);
        let zero = dir.join("zero.pid");
        std::fs::write(&zero, "0\n").unwrap();
        assert_eq!(read_pid(&zero), None);
        let real = dir.join("real.pid");
        std::fs::write(&real, "  4321\n").unwrap();
        assert_eq!(read_pid(&real), Some(4321));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
