//! What the Mac's `setup`, `update` and `import` do for the window, as Linux
//! does it: the package manager installs and updates Kiri, the engine is a
//! system service, and there is no OrbStack to import from.

/// Why the window can't reach the engine, and what to do about it.
pub fn engine_down() -> std::io::Error {
    let socket = std::path::Path::new(crate::client::SERVICE_SOCKET);
    std::io::Error::other(match std::os::unix::net::UnixStream::connect(socket) {
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            "This session can't reach Kiri yet: log out and back in once, so your account's new kiri group applies"
        }
        _ if socket.exists() => "Kiri's engine isn't answering: sudo systemctl restart kiri",
        _ => "Kiri's engine isn't running: sudo systemctl enable --now kiri",
    })
}

pub mod setup {
    use std::io;

    /// The docker CLI comes from the distribution; nothing to set up after it.
    pub fn after_tools() -> io::Result<()> {
        Ok(())
    }

    /// `docker build` through Kiri's BuildKit builder, as on the Mac (needs
    /// the docker-buildx package, which the .deb pulls in).
    pub use crate::builds::ensure_builder;

    pub fn domains() -> io::Result<()> {
        Err(io::Error::other("*.kiri.local isn't on Linux yet; containers answer at <name>.kiri.localhost"))
    }

    pub fn remove_domains() -> io::Result<()> {
        Ok(())
    }

    /// The package's service provides /var/run/docker.sock (kiri.service).
    pub fn docker_socket_note() -> Option<String> {
        None
    }

    /// Which engine plain `docker` uses: the CLI's own switch.
    pub fn use_context(name: &str) -> io::Result<()> {
        if !name.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b)) {
            return Err(io::Error::other("not a context name"));
        }
        let out = std::process::Command::new("docker").args(["context", "use", name]).output()?;
        if out.status.success() { Ok(()) } else { Err(io::Error::other(String::from_utf8_lossy(&out.stderr).trim().to_owned())) }
    }
}

pub mod update {
    use std::io;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;

    const BY_APT: &str = "on Linux, Kiri updates with your other packages (apt or dnf)";

    #[derive(serde::Serialize)]
    pub struct Release {
        pub version: String,
    }

    /// Whether `candidate` is a later version than `current`, as on the Mac.
    pub fn newer(candidate: &str, current: &str) -> bool {
        let parts = |v: &str| -> Vec<u64> { v.trim_start_matches('v').split(['.', '-']).map(|p| p.parse().unwrap_or(0)).collect() };
        parts(candidate) > parts(current)
    }

    pub fn latest() -> io::Result<Release> {
        Err(io::Error::other(BY_APT))
    }

    pub fn download(_: &Release, _: &mut dyn FnMut(u64, Option<u64>), _: &AtomicBool) -> io::Result<()> {
        Err(io::Error::other(BY_APT))
    }

    pub fn staged() -> Option<String> {
        None
    }

    pub fn install_staged() -> io::Result<String> {
        Err(io::Error::other(BY_APT))
    }

    pub fn app() -> io::Result<PathBuf> {
        std::env::current_exe()
    }

    /// The engine is the `kiri` system service, so restarting it is root's:
    /// `kiri host-access --restart` through pkexec (one password prompt),
    /// then the engine must answer as `expect`.
    pub fn restart_engine(expect: &str) -> io::Result<String> {
        crate::diag::info!("restarting the engine", version = expect);
        super::onboarding::restart_engine()?;
        let began = std::time::Instant::now();
        let mut last = None;
        while began.elapsed() < std::time::Duration::from_secs(60) {
            if let Some(v) = crate::client::request("GET", "/version", None).ok().filter(|r| r.ok()).and_then(|r| crate::client::kiri_version(&r.json()).map(str::to_owned)) {
                if v == expect {
                    return Ok(v);
                }
                last = Some(v);
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        Err(io::Error::other(match last {
            Some(v) => format!("the engine came back on {v}, not {expect}: is the kiri package upgraded?"),
            None => "the engine did not come back: journalctl -u kiri says why".to_owned(),
        }))
    }

    /// Never called: the window doesn't start Linux's engine (systemd does).
    pub fn start_engine() -> io::Result<()> {
        Err(super::engine_down())
    }
}

pub mod import {
    use std::io;

    use serde_json::Value;

    /// OrbStack and Docker Desktop are the Mac's; on Linux there's none to list.
    pub fn sources() -> Vec<Value> {
        Vec::new()
    }

    fn none() -> io::Error {
        io::Error::other("importing isn't on Linux")
    }

    pub fn inventory(_: &str) -> io::Result<Value> {
        Err(none())
    }

    pub fn image(_: &str, _: &str) -> io::Result<()> {
        Err(none())
    }

    pub fn volume(_: &str, _: &str) -> io::Result<()> {
        Err(none())
    }

    pub fn container(_: &str, _: &str) -> io::Result<()> {
        Err(none())
    }
}

/// The checklist, and `kiri setup` for it in a terminal.
pub(crate) use crate::onboarding;
