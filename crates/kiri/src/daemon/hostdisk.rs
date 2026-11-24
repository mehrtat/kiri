//! Room on the host drive that holds this engine's own disk, as kiri-win
//! last reported it (`PUT /kiri/hostdisk`). On Windows the distro's
//! ext4.vhdx grows on a drive Linux cannot see: when that drive fills, the
//! vhdx's writes fail, processes die of SIGBUS and ext4 goes read-only,
//! and the disk stays damaged after a restart. Below the floor, the engine
//! refuses new work (create, start, pull, load, import, build) with an
//! answer that names the drive, before that happens.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The least room the drive may have for new work to start.
pub const FLOOR: u64 = 5_000_000_000;
/// How long a report counts: kiri-win sends one every 30 s, and one from a
/// kiri-win that is gone refuses nothing.
const FRESH: Duration = Duration::from_secs(120);

struct Report {
    at: Instant,
    drive: String,
    free: u64,
}

static LAST: Mutex<Option<Report>> = Mutex::new(None);

pub fn set(drive: &str, free: u64) {
    *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some(Report { at: Instant::now(), drive: drive.to_owned(), free });
}

/// Whether a request writes to the engine's disk as new work.
fn new_work(method: &str, path: &str) -> bool {
    method == "POST"
        && (matches!(path, "/containers/create" | "/images/create" | "/images/load" | "/build")
            || path.strip_prefix("/containers/").and_then(|r| r.strip_suffix("/start")).is_some_and(|id| !id.is_empty() && !id.contains('/')))
}

/// Why this request is refused, if it is.
pub fn refusal(method: &str, path: &str) -> Option<String> {
    if !new_work(method, path) {
        return None;
    }
    let last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    last.as_ref().and_then(|r| refusal_of(r, Instant::now()))
}

fn refusal_of(r: &Report, now: Instant) -> Option<String> {
    (now.saturating_duration_since(r.at) < FRESH && r.free < FLOOR).then(|| {
        format!(
            "host drive {d} is full: {} MB free, below Kiri's floor of {} MB. Kiri's Linux disk lives on {d}; free space there before starting more work",
            r.free / 1_000_000,
            FLOOR / 1_000_000,
            d = r.drive
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{new_work, refusal_of, Report, FLOOR, FRESH};
    use std::time::Instant;

    #[test]
    fn only_new_work_is_refused() {
        for (m, p) in [("POST", "/containers/create"), ("POST", "/containers/abc/start"), ("POST", "/images/create"), ("POST", "/images/load"), ("POST", "/build")] {
            assert!(new_work(m, p), "{m} {p}");
        }
        for (m, p) in [("GET", "/containers/json"), ("POST", "/containers/abc/stop"), ("DELETE", "/containers/abc"), ("POST", "/images/prune"), ("POST", "/containers//start"), ("POST", "/build/prune")] {
            assert!(!new_work(m, p), "{m} {p}");
        }
    }

    #[test]
    fn a_full_drive_refuses_and_names_itself() {
        let now = Instant::now();
        let r = Report { at: now, drive: "C:".into(), free: 1_200_000_000 };
        let m = refusal_of(&r, now).expect("refused");
        assert!(m.starts_with("host drive C: is full: 1200 MB free"), "{m}");
        assert!(refusal_of(&Report { free: FLOOR, ..r }, now).is_none());
    }

    #[test]
    fn an_old_report_refuses_nothing() {
        let now = Instant::now();
        let r = Report { at: now, drive: "C:".into(), free: 0 };
        assert!(refusal_of(&r, now + FRESH).is_none());
    }
}
