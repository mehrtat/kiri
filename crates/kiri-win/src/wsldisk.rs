//! Kiri's Linux disk failing under the engine. When the drive that holds the
//! distro's ext4.vhdx runs out of room, or the disk is damaged, ext4 goes
//! read-only (the daemon answers "Read-only file system (os error 30)") and
//! then reads fail too: wsl.exe answers `getpwnam(root) failed 5`
//! (Wsl/WSL_E_USER_NOT_FOUND) and the engine cannot start. Restarting WSL
//! does not cure it, and "run kiri.ps1 to install it" sent people the wrong
//! way. Plain code, tested on any host.

/// Below this much room on the vhdx's drive, the next write that grows the
/// disk can fail: the daemon's floor for new work (daemon/hostdisk.rs).
pub const LOW_FREE: u64 = 5_000_000_000;

/// The window's warning when the drive runs low.
pub fn low_notice(drive: &str, free: u64) -> String {
    format!("The drive {drive} that holds Kiri's Linux disk has {} MB free: Kiri starts no new containers, pulls or builds until there is more room on {drive}", free / 1_000_000)
}

/// Whether what wsl.exe said, starting the engine, means the distro's disk
/// failed.
pub fn failed(said: &str) -> bool {
    reason(said).is_some()
}

/// The sign in what wsl.exe said that the distro's disk failed, if any.
pub fn reason(said: &str) -> Option<&'static str> {
    if said.contains("getpwnam(") && said.contains("failed 5") {
        return Some("getpwnam failed 5 (input/output error)");
    }
    [("WSL_E_USER_NOT_FOUND", "WSL_E_USER_NOT_FOUND"), ("Read-only file system", "read-only file system"), ("Input/output error", "input/output error")]
        .iter()
        .find(|(sign, _)| said.contains(sign))
        .map(|(_, why)| *why)
}

/// The least time between two automatic rebuilds: a disk that fails again
/// at once has a cause a rebuild does not cure.
pub const REBUILD_EVERY: u64 = 60 * 60;

/// Whether kiri-win rebuilds the failed distro on its own (`autoRebuild`,
/// for unattended machines whose engine data is disposable). Only Kiri
/// Linux, never a distro of the person's own; only with the drive's room
/// known and above the floor, since a rebuild on a full drive fails the
/// same way; at most once in [`REBUILD_EVERY`].
pub fn may_rebuild(on: bool, kiri_linux: bool, free: Option<u64>, last: Option<u64>, now: u64) -> bool {
    on && kiri_linux && free.is_some_and(|f| f >= LOW_FREE) && last.is_none_or(|at| now.saturating_sub(at) >= REBUILD_EVERY)
}

/// The line the boot log gets once a rebuild worked, for scripts to match.
pub fn rebuilt_line(why: &str) -> String {
    format!("engine: Linux disk rebuilt after failure: {why}")
}

/// The error shown when the engine did not start. `drive` is the drive
/// that holds the distro's disk and its free bytes, when known. Each
/// message about the disk starts "Kiri's Linux disk failed", for scripts
/// that sort engine faults.
pub fn not_started(said: &str, distro: &str, drive: Option<(&str, u64)>) -> String {
    if !failed(said) {
        return "Kiri's engine did not start in WSL: run contrib\\windows\\kiri.ps1 to install it".to_owned();
    }
    match drive {
        Some((d, free)) if free < LOW_FREE => format!(
            "Kiri's Linux disk failed: the drive {d} that holds it (WSL distro {distro}) has {} MB free. Free space on {d}, then run wsl --shutdown and open Kiri again",
            free / 1_000_000
        ),
        _ => format!(
            "Kiri's Linux disk failed (WSL distro {distro}: read-only or input/output errors). Repair it with e2fsck from another distro after wsl --mount --vhd <its ext4.vhdx> --bare, or remove it with wsl --unregister {distro} (its images and volumes are lost) and open Kiri to set it up again"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{failed, low_notice, may_rebuild, not_started, reason, rebuilt_line, LOW_FREE, REBUILD_EVERY};

    #[test]
    fn each_disk_failure_is_seen() {
        assert!(failed("CreateProcessParseCommon:1061: getpwnam(root) failed 5\r\nError code: Wsl/WSL_E_USER_NOT_FOUND"));
        assert!(failed("getpwnam(root) failed 5"));
        assert!(failed("Error code: Wsl/WSL_E_USER_NOT_FOUND"));
        assert!(failed("mkdir: can't create directory '/var/log': Read-only file system"));
        assert!(failed("sh: /usr/local/bin/kiri-start: Input/output error"));
    }

    #[test]
    fn other_failures_are_not_the_disk() {
        assert!(!failed(""));
        assert!(!failed("There is no distribution with the supplied name."));
        assert!(!failed("getpwnam(kiri) failed 0"));
        assert!(not_started("", "kiri", None).contains("kiri.ps1"));
    }

    #[test]
    fn a_full_drive_is_named() {
        let m = not_started("getpwnam(root) failed 5", "kiri", Some(("C:", 300_000_000)));
        assert!(m.starts_with("Kiri's Linux disk failed"), "{m}");
        assert!(m.contains("C:") && m.contains("300 MB free"), "{m}");
    }

    #[test]
    fn the_low_warning_names_the_drive() {
        assert!(low_notice("C:", 4_200_000_000).starts_with("The drive C: that holds Kiri's Linux disk has 4200 MB free"));
    }

    #[test]
    fn a_damaged_disk_says_how_to_repair_it() {
        for drive in [None, Some(("D:", LOW_FREE))] {
            let m = not_started("Read-only file system", "kiri", drive);
            assert!(m.starts_with("Kiri's Linux disk failed"), "{m}");
            assert!(m.contains("e2fsck") && m.contains("wsl --unregister kiri"), "{m}");
        }
    }

    #[test]
    fn the_reason_names_the_first_sign() {
        assert_eq!(reason("CreateProcessParseCommon:1061: getpwnam(root) failed 5\r\nError code: Wsl/WSL_E_USER_NOT_FOUND"), Some("getpwnam failed 5 (input/output error)"));
        assert_eq!(reason("touch: /x: Read-only file system"), Some("read-only file system"));
        assert_eq!(reason("all fine"), None);
        assert_eq!(rebuilt_line("read-only file system"), "engine: Linux disk rebuilt after failure: read-only file system");
    }

    #[test]
    fn a_rebuild_needs_the_setting_kiri_linux_room_and_time() {
        let now = 10 * REBUILD_EVERY;
        assert!(may_rebuild(true, true, Some(LOW_FREE), None, now));
        assert!(may_rebuild(true, true, Some(LOW_FREE), Some(now - REBUILD_EVERY), now));
        assert!(!may_rebuild(false, true, Some(LOW_FREE), None, now), "off by default");
        assert!(!may_rebuild(true, false, Some(LOW_FREE), None, now), "never a distro of the person's own");
        assert!(!may_rebuild(true, true, Some(LOW_FREE - 1), None, now), "a full drive fails a rebuild too");
        assert!(!may_rebuild(true, true, None, None, now), "room unknown");
        assert!(!may_rebuild(true, true, Some(LOW_FREE), Some(now - 60), now), "rebuilt a minute ago");
    }
}
