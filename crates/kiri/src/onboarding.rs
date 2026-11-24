//! What the window checks when it opens on a Linux host, before anything
//! else, and how it fixes each: the package installs the engine but can't
//! start it for, or give access to, the person who'll use it. The page shows
//! the checklist (index.html, `onboarding`) until every required check passes,
//! so the main window never opens onto an engine it can't reach.

use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

const SOCKET: &str = crate::client::SERVICE_SOCKET;
/// The package's command; pkexec wants a full path.
const KIRI: &str = "/usr/bin/kiri";

/// Each check's state for the page: "ok", "no" (fix it), "bad" (broken:
/// fix it), "off" (can't tell until an earlier one passes) or "restart"
/// (fixed, applies after signing in again). `ready` when Kiri can run
/// containers for this account; docker and builds are offered, not required
/// (someone may keep docker on another engine on purpose).
pub fn checks() -> serde_json::Value {
    let installed = ["/lib/systemd/system/kiri.service", "/usr/lib/systemd/system/kiri.service", "/etc/systemd/system/kiri.service"]
        .iter()
        .any(|p| Path::new(p).exists());
    let (running, access) = match std::os::unix::net::UnixStream::connect(SOCKET) {
        Ok(_) => ("ok", "ok"),
        // The engine is there; this session may not use it. In the group
        // already but refused: the group applies at the next sign-in, and
        // the access list that would have let it in now isn't there.
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => ("ok", if in_group_not_session() { "restart" } else { "no" }),
        Err(_) if Path::new(SOCKET).exists() => ("bad", "off"),
        Err(_) => ("no", "off"),
    };
    let usable = access == "ok";
    let docker = if !usable { "off" } else if docker_uses_kiri() { "ok" } else { "no" };
    let builds = if !usable { "off" } else if builder_made() { "ok" } else { "no" };
    let ready = installed && usable;
    // Setup is done when docker and builds pass too, or someone chose to
    // skip them (docker kept on another engine on purpose): until then
    // the checklist stays and Open Kiri waits.
    let skipped = skipped();
    let done = |k: &str, state: &str| state == "ok" || skipped.iter().any(|s| s == k);
    let complete = ready && done("docker", docker) && done("builds", builds);
    serde_json::json!({
        "type": "onboarding",
        "ready": ready,
        "complete": complete,
        "skipped": skipped,
        "user": user(),
        "checks": {"engine": if installed { "ok" } else { "no" }, "running": running, "access": access, "docker": docker, "builds": builds},
    })
}

/// Whether to show the checklist at all: setup isn't complete.
pub fn needed(checks: &serde_json::Value) -> bool {
    checks["complete"] != true
}

/// The optional checks someone chose to skip ("docker", "builds"), kept
/// in the window's settings (~/.kiri/ui.json, `setupSkip`).
fn skipped() -> Vec<String> {
    settings_path().map(|p| settings(&p)).unwrap_or_default()["setupSkip"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_owned)).collect()
}

/// The window's settings file, as the window finds it (ui::state_dir): the
/// account's ~/.kiri on a host with the system service.
fn settings_path() -> Option<std::path::PathBuf> {
    let dir = match std::env::var_os("KIRI_SOCKET") {
        None => Path::new(&std::env::var_os("HOME")?).join(crate::flavor::STATE.get()),
        Some(_) => crate::client::socket().ok()?.parent()?.parent()?.to_path_buf(),
    };
    Some(dir.join("ui.json"))
}

fn settings(path: &Path) -> serde_json::Value {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).filter(serde_json::Value::is_object).unwrap_or_else(|| serde_json::json!({}))
}

/// Saves the skipped checks where the window keeps them, so it and
/// `kiri setup` agree.
fn save_skipped(path: &Path, list: serde_json::Value) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut all = settings(path);
    all["setupSkip"] = list;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, all.to_string())?;
    std::fs::rename(&tmp, path)
}

/// Skips one optional check, or takes the skip back (`undo`).
pub fn skip(what: &str, undo: bool) -> serde_json::Value {
    let mut list = skipped();
    list.retain(|s| s != what);
    if !undo && matches!(what, "docker" | "builds") {
        list.push(what.to_owned());
    }
    serde_json::json!(list)
}

/// Fixes one check, or "all" of them; Err says why it couldn't. Root's
/// part (the engine, access) is one `kiri host-access` through pkexec, so
/// the system asks for the password once.
pub fn fix(what: &str) -> io::Result<()> {
    let c = checks();
    let needs = |k: &str| c["checks"][k] != "ok";
    if matches!(what, "running" | "access" | "all") && (needs("running") || needs("access")) {
        host_access()?;
    }
    if matches!(what, "docker" | "all") && !docker_uses_kiri() {
        use_kiri()?;
    }
    if matches!(what, "builds" | "all") && !builder_made() {
        crate::builds::builder()?;
    }
    Ok(())
}

fn host_access() -> io::Result<()> {
    // `sudo kiri setup`: root already, for the account that ran sudo.
    if is_root() {
        let user = std::env::var("SUDO_USER").ok().filter(|u| !u.is_empty() && u != "root").ok_or_else(|| io::Error::other("as root, Kiri can't tell whose engine to set up: run `kiri setup` from that account (it asks for the password)"))?;
        return crate::hostaccess::run(std::slice::from_ref(&user)).map(|_| ());
    }
    pkexec_host_access(&[])
}

fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() == 0 }
}

/// Restarts the `kiri` service, through the same password prompt.
pub fn restart_engine() -> io::Result<()> {
    pkexec_host_access(&["--restart"])
}

fn pkexec_host_access(flags: &[&str]) -> io::Result<()> {
    let user = user();
    if which("pkexec").is_none() {
        return Err(io::Error::other(format!("this system has no pkexec to ask for your password: in a terminal, run  sudo kiri host-access {}{user}", flags.iter().map(|f| format!("{f} ")).collect::<String>())));
    }
    let out = Command::new("pkexec").arg(KIRI).arg("host-access").args(flags).arg(&user).stdin(Stdio::null()).output()?;
    match out.status.code() {
        Some(0) | Some(10) => Ok(()),
        // pkexec: 126 the dialog was dismissed or the password refused, 127 not allowed.
        Some(126) | Some(127) => Err(io::Error::other("Not changed: the password prompt was cancelled")),
        _ => {
            let why = String::from_utf8_lossy(&out.stderr);
            let why = why.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("kiri host-access failed").trim_start_matches("kiri: ");
            Err(io::Error::other(why.to_owned()))
        }
    }
}

/// `docker` reaches Kiri: its current endpoint is Kiri's socket, or the
/// standard socket that leads to it. Kiri's own `docker` (the package's
/// wrapper) does unless someone chose another context.
pub fn docker_uses_kiri() -> bool {
    let Some(docker) = crate::client::docker_cli() else { return false };
    let Ok(out) = Command::new(docker).args(["context", "inspect", "--format", "{{.Endpoints.docker.Host}}"]).stderr(Stdio::null()).output() else { return false };
    let host = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    match host.strip_prefix("unix://") {
        Some(p) if p == SOCKET => true,
        Some(p) => std::fs::canonicalize(p).is_ok_and(|real| real == Path::new(SOCKET)),
        None => false,
    }
}

/// Kiri's docker context (`kiri`, or a development flavor's), made if
/// missing, as the current one.
fn use_kiri() -> io::Result<()> {
    let docker = crate::client::docker_cli().ok_or_else(|| io::Error::other("no docker command here: install the kiri package (not kiri-lite) or docker's CLI"))?;
    let name = crate::flavor::CONTEXT.get();
    let _ = Command::new(&docker).args(["context", "create", name, "--docker", &format!("host=unix://{SOCKET}")]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    let out = Command::new(&docker).args(["context", "use", name]).output()?;
    if out.status.success() { Ok(()) } else { Err(io::Error::other(String::from_utf8_lossy(&out.stderr).trim().to_owned())) }
}

fn builder_made() -> bool {
    let dir = std::env::var_os("BUILDX_CONFIG").map(std::path::PathBuf::from).or_else(|| {
        let config = std::env::var_os("DOCKER_CONFIG").map(std::path::PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".docker")))?;
        Some(config.join("buildx"))
    });
    dir.is_some_and(|d| d.join("instances").join(crate::builds::BUILDER.get()).exists())
}

/// In the kiri group by /etc/group, but this process (this session) isn't.
fn in_group_not_session() -> bool {
    let me = user();
    let listed = crate::daemon::access::members("kiri").is_ok_and(|m| m.contains(&me));
    listed && !session_has_group("kiri")
}

fn session_has_group(group: &str) -> bool {
    let Ok(name) = std::ffi::CString::new(group) else { return false };
    // SAFETY: getgrnam with a valid C string, read at once; getgroups
    // fills at most the buffer's length.
    unsafe {
        let g = libc::getgrnam(name.as_ptr());
        if g.is_null() {
            return false;
        }
        let gid = (*g).gr_gid;
        if libc::getegid() == gid {
            return true;
        }
        let mut groups = vec![0 as libc::gid_t; 256];
        let n = libc::getgroups(groups.len() as libc::c_int, groups.as_mut_ptr());
        n > 0 && groups[..n as usize].contains(&gid)
    }
}

fn user() -> String {
    // SAFETY: getpwuid on our own uid; the name is copied at once.
    let from_uid = unsafe {
        let p = libc::getpwuid(libc::getuid());
        (!p.is_null()).then(|| std::ffi::CStr::from_ptr((*p).pw_name).to_string_lossy().into_owned())
    };
    from_uid.or_else(|| std::env::var("USER").ok()).unwrap_or_default()
}

fn which(cmd: &str) -> Option<std::path::PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(cmd)).find(|p| p.is_file())
}

/// Ends this session, so the next sign-in has the kiri group: the
/// desktop's own log-out (it asks to confirm), else logind.
pub fn log_out() -> io::Result<()> {
    if which("gnome-session-quit").is_some() && Command::new("gnome-session-quit").arg("--logout").status()?.success() {
        return Ok(());
    }
    if let Some(qdbus) = ["qdbus6", "qdbus"].iter().find(|q| which(q).is_some())
        && Command::new(qdbus).args(["org.kde.Shutdown", "/Shutdown", "logout"]).status()?.success()
    {
        return Ok(());
    }
    let session = std::env::var("XDG_SESSION_ID").map_err(|_| io::Error::other("couldn't tell which session to end: log out from the system menu"))?;
    if Command::new("loginctl").args(["terminate-session", &session]).status()?.success() { Ok(()) } else { Err(io::Error::other("couldn't log out: use the system menu")) }
}

pub fn restart() -> io::Result<()> {
    if which("gnome-session-quit").is_some() && Command::new("gnome-session-quit").arg("--reboot").status()?.success() {
        return Ok(());
    }
    if Command::new("systemctl").arg("reboot").status()?.success() { Ok(()) } else { Err(io::Error::other("couldn't restart: use the system menu")) }
}

const USAGE: &str = "usage: kiri setup [status] | all | running | access | docker | builds | skip docker|builds | unskip docker|builds\n\
    The window's first-run checklist, from a terminal: `kiri setup all` does every step\n\
    (the engine and access ask for your password once; docker and builds are your account's).";

/// `kiri setup`: the checklist the window shows on its first start, and
/// its fixes, with no window: for a server, a script, or the static build
/// that has none.
pub fn cli(args: &[String]) -> io::Result<()> {
    let usage = || io::Error::new(io::ErrorKind::InvalidInput, USAGE);
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [] | ["status"] => {}
        [verb @ ("skip" | "unskip"), what @ ("docker" | "builds")] => {
            let path = settings_path().ok_or_else(|| io::Error::other("no home directory for Kiri's settings"))?;
            save_skipped(&path, skip(what, verb == "unskip"))?;
        }
        [what @ ("all" | "running" | "access" | "docker" | "builds")] => {
            let user_steps = matches!(what, "docker" | "builds" | "all");
            if is_root() && what != "all" && user_steps {
                return Err(io::Error::other(format!("`kiri setup {what}` sets up your own docker: run it without sudo")));
            }
            // As root, only root's part: docker and builds are the account's.
            fix(if is_root() { "access" } else { what })?;
            if is_root() && what == "all" {
                println!("Engine and access done; run `kiri setup all` without sudo for docker and builds.");
            }
        }
        _ => return Err(usage()),
    }
    print!("{}", report(&checks()));
    Ok(())
}

/// The checklist as text, each check with what to do about it.
fn report(c: &serde_json::Value) -> String {
    let skipped: Vec<&str> = c["skipped"].as_array().into_iter().flatten().filter_map(|v| v.as_str()).collect();
    let mut out = format!("Kiri setup for {}\n", c["user"].as_str().unwrap_or_default());
    for (key, label) in [("engine", "engine installed"), ("running", "engine running"), ("access", "access"), ("docker", "docker uses Kiri"), ("builds", "Kiri's builder")] {
        let state = c["checks"][key].as_str().unwrap_or("off");
        let state = if state != "ok" && skipped.contains(&key) { "skipped" } else { state };
        let hint = match (key, state) {
            (_, "ok" | "skipped" | "off") => String::new(),
            ("engine", _) => "install the kiri package".into(),
            (_, "restart") => "log out and back in once, so the kiri group applies".into(),
            (k, _) => format!("kiri setup {k}"),
        };
        out.push_str(format!("  {label:<18} {state:<8} {hint}").trim_end());
        out.push('\n');
    }
    out.push_str(if c["complete"] == true {
        "Setup complete.\n"
    } else if c["ready"] == true {
        "Kiri runs containers; the rest is optional (`kiri setup skip docker|builds` to leave one).\n"
    } else {
        "Not done yet: kiri setup all\n"
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_says_what_to_run_for_each_check() {
        let c = serde_json::json!({
            "user": "sam", "ready": true, "complete": false, "skipped": ["builds"],
            "checks": {"engine": "ok", "running": "ok", "access": "restart", "docker": "no", "builds": "no"},
        });
        let r = report(&c);
        assert!(r.starts_with("Kiri setup for sam\n"), "{r}");
        assert!(r.contains("access             restart  log out and back in once"), "{r}");
        assert!(r.contains("docker uses Kiri   no       kiri setup docker"), "{r}");
        assert!(r.contains("Kiri's builder     skipped\n"), "{r}");
        assert!(r.contains("engine running     ok\n"), "{r}");
        assert!(r.ends_with("the rest is optional (`kiri setup skip docker|builds` to leave one).\n"), "{r}");
        let none = serde_json::json!({"user": "sam", "ready": false, "complete": false, "skipped": [], "checks": {"engine": "no", "running": "no", "access": "off", "docker": "off", "builds": "off"}});
        let r = report(&none);
        assert!(r.contains("engine installed   no       install the kiri package"), "{r}");
        assert!(r.contains("access             off\n"), "{r}");
        assert!(r.ends_with("Not done yet: kiri setup all\n"), "{r}");
    }

    #[test]
    fn skips_are_kept_with_the_windows_other_settings() {
        let path = std::env::temp_dir().join(format!("kiri-setup-skip-{}/ui.json", std::process::id()));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        save_skipped(&path, serde_json::json!(["docker"])).unwrap();
        assert_eq!(settings(&path)["setupSkip"], serde_json::json!(["docker"]));
        std::fs::write(&path, r#"{"theme":"light","setupSkip":["docker"]}"#).unwrap();
        save_skipped(&path, serde_json::json!([])).unwrap();
        assert_eq!(settings(&path), serde_json::json!({"theme": "light", "setupSkip": []}));
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(settings(&path), serde_json::json!({}));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
