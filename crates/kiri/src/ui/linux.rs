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

    /// Where Kiri's daemon listens on Linux (`KIRI_SOCKET` or `/run/kiri/docker.sock`).
    pub fn socket_path() -> std::path::PathBuf {
        std::env::var_os("KIRI_SOCKET").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from(crate::client::SERVICE_SOCKET))
    }

    /// Who owns `/var/run/docker.sock` on Linux.
    pub fn docker_sock() -> crate::dockerctx::Endpoint {
        crate::dockerctx::sock_at(std::path::Path::new("/var/run/docker.sock"), &socket_path(), &[])
    }

    /// Settings → Network's Docker socket row on Linux (status-only: `kiri.service` links it).
    pub fn docker_sock_event() -> serde_json::Value {
        let ours = format!("unix://{}", socket_path().display());
        crate::dockerctx::sock_event("/var/run/docker.sock", &docker_sock(), &ours, false)
    }

    /// Unused on Linux (`take` is false), kept to share `ui::Cmd::TakeSocket`.
    pub fn docker_socket() -> io::Result<()> {
        Ok(())
    }

    /// The package's service provides /var/run/docker.sock (kiri.service).
    pub fn docker_socket_note() -> Option<String> {
        None
    }

    /// Whether `docker` resolves to Kiri's packaged wrapper while no context
    /// is stored in config.json (`DOCKER_HOST` / `DOCKER_CONTEXT` handled first).
    fn wrapper_active() -> bool {
        let Some(docker) = crate::client::docker_cli() else { return false };
        let Ok(real) = std::fs::canonicalize(docker) else { return false };
        if real != std::path::Path::new("/usr/libexec/kiri/docker") {
            return false;
        }
        wrapper_uses_default_config(crate::dockerctx::docker_dir().as_deref())
    }

    fn wrapper_uses_default_config(dir: Option<&std::path::Path>) -> bool {
        let Some(dir) = dir else { return true };
        let Ok(bytes) = std::fs::read(dir.join("config.json")) else { return true };
        let Ok(cfg) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return true };
        cfg.get("currentContext").and_then(|v| v.as_str()).is_none()
    }

    /// What plain `docker` uses on Linux, factoring in Kiri's wrapper script.
    pub fn current_cli(endpoint: &crate::dockerctx::Endpoint) -> crate::dockerctx::Cli {
        let ours = format!("unix://{}", socket_path().display());
        let mut cli = crate::dockerctx::current_cli(crate::flavor::CONTEXT.get(), &[&ours], endpoint);
        if !cli.kiri && cli.context == "default" && cli.pinned.is_none() && wrapper_active() {
            cli.kiri = true;
        }
        cli
    }

    /// Which engine plain `docker` uses: Kiri's context (created if missing),
    /// "default", or another existing context, preserving logins and plugins.
    pub fn use_context(name: &str) -> io::Result<()> {
        let dir = crate::dockerctx::docker_dir().ok_or_else(|| io::Error::other("HOME is not set"))?;
        let digest = kiri_image::store::sha256_digest(name.as_bytes());
        let meta = dir.join("contexts/meta").join(digest.trim_start_matches("sha256:"));
        let host = format!("unix://{}", socket_path().display());
        crate::dockerctx::switch(&dir, &meta, name, crate::flavor::CONTEXT.get(), crate::flavor::APP.get(), &host)
    }

    fn config_dir() -> Option<std::path::PathBuf> {
        let xdg = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from);
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        crate::ui::xdg_config_dir(xdg.as_deref(), home.as_deref())
    }

    pub fn start_at_login() -> bool {
        config_dir().is_some_and(|d| crate::ui::linux_autostart_at(&d))
    }

    pub fn set_start_at_login(on: bool) -> io::Result<()> {
        let dir = config_dir().ok_or_else(|| io::Error::other("HOME is not set"))?;
        crate::ui::set_linux_autostart_at(&dir, on)
    }

    pub fn bootout_agent() {}
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

    pub fn stop_engine() -> io::Result<()> {
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::setup;
    use crate::dockerctx::{Endpoint, sock_event};

    #[test]
    fn linux_sock_event_is_status_only() {
        let e = sock_event("/var/run/docker.sock", &Endpoint::Free, "unix:///run/kiri/docker.sock", false);
        assert_eq!((e["owner"].as_str(), e["take"].as_bool()), (Some("free"), Some(false)));
        let e = sock_event("/var/run/docker.sock", &Endpoint::Taken("/run/podman/podman.sock".into()), "unix:///run/kiri/docker.sock", false);
        assert_eq!((e["owner"].as_str(), e["engine"].as_str(), e["take"].as_bool()), (Some("other"), Some("Podman"), Some(false)));
        let _ = setup::docker_socket();
    }
}
