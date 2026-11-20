//! Taking the pipe over from a kiri-win that is still going away. A script
//! that stops Kiri and starts it again at once (`Stop-Process`, then
//! `kiri-win --background`) starts the new one while Windows is still
//! closing the old one's handles: the pipe's name is still held, the new one
//! took that for a running Kiri and quit, and a moment later no kiri-win was
//! left at all (docker: "open //./pipe/kiri: The system cannot find the file
//! specified"), for hours on a gate PC. Plain code, tested on any host.

use std::io;
use std::time::{Duration, Instant};

/// Calls `start` until it succeeds. While the name is held
/// (`AddrInUse`) by something that is not a Kiri answering its window
/// (`kiri_answers`: one being killed, or hung, does not), it tries again
/// every `pause` for up to `patience`; otherwise the error comes back.
pub fn start_when_free(mut start: impl FnMut() -> io::Result<()>, mut kiri_answers: impl FnMut() -> bool, patience: Duration, pause: Duration) -> io::Result<()> {
    let deadline = Instant::now() + patience;
    loop {
        match start() {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() != io::ErrorKind::AddrInUse || kiri_answers() || Instant::now() >= deadline => return Err(e),
            Err(_) => std::thread::sleep(pause),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::start_when_free;
    use std::io;
    use std::time::Duration;

    fn held() -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::AddrInUse, "held"))
    }

    const PATIENCE: Duration = Duration::from_secs(5);
    const PAUSE: Duration = Duration::from_millis(1);

    #[test]
    fn a_kiri_still_going_away_is_waited_for() {
        let mut tries = 0;
        let r = start_when_free(
            || {
                tries += 1;
                if tries < 4 { held() } else { Ok(()) }
            },
            || false,
            PATIENCE,
            PAUSE,
        );
        assert!(r.is_ok());
        assert_eq!(tries, 4);
    }

    #[test]
    fn a_kiri_that_answers_is_left_to_serve() {
        let mut tries = 0;
        let r = start_when_free(
            || {
                tries += 1;
                held()
            },
            || true,
            PATIENCE,
            PAUSE,
        );
        assert_eq!(r.unwrap_err().kind(), io::ErrorKind::AddrInUse);
        assert_eq!(tries, 1);
    }

    #[test]
    fn a_name_held_for_good_gives_up_in_time() {
        let r = start_when_free(held, || false, Duration::from_millis(30), PAUSE);
        assert_eq!(r.unwrap_err().kind(), io::ErrorKind::AddrInUse);
    }

    #[test]
    fn another_failure_is_not_tried_again() {
        let mut tries = 0;
        let r = start_when_free(
            || {
                tries += 1;
                Err(io::Error::other("no descriptor"))
            },
            || false,
            PATIENCE,
            PAUSE,
        );
        assert!(r.is_err());
        assert_eq!(tries, 1);
    }
}
