//! Docker Hub through its mirrors. Where Docker Hub itself can't be reached
//! (it refuses pulls from Iran, and rate-limits every anonymous address), a
//! pull of a Hub image goes to the mirror that answered fastest, and to the
//! next one when that fails. Only Docker Hub is mirrored, as with dockerd's
//! `registry-mirrors`; credentials go only to Hub itself, never to a mirror.
//!
//! A mirror serves a tag as it likes, as with Docker's own mirrors: a pull
//! by digest is checked against the digest, a pull by tag is not.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Docker Hub's own API host, tried like any mirror.
pub const ORIGIN: &str = "registry-1.docker.io";

/// Hub mirrors that served `library/alpine:latest` with Hub's own digest
/// from Iran on 2026-11-19: Iranian ones first, then others abroad.
pub const BUILT_IN: &[&str] = &[
    "docker.arvancloud.ir",
    "docker-mirror.liara.ir",
    "focker.ir",
    "hub.hamdocker.ir",
    "docker.iranserver.com",
    "docker.mobinhost.com",
    "docker.kernel.ir",
    "registry.docker.ir",
    "mirror.gcr.io",
    "dockerhub.timeweb.cloud",
    "docker.m.daocloud.io",
];

/// How long a ranking is trusted before the mirrors are timed again.
const RANK_FOR: Duration = Duration::from_secs(10 * 60);

/// What `kiri registry mirror` keeps: mirrors of one's own, tried before
/// the built-in ones, and whether to use those at all.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub mirrors: Vec<String>,
    #[serde(default = "yes")]
    pub builtin: bool,
}

fn yes() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Settings { mirrors: Vec::new(), builtin: true }
    }
}

/// Where the settings live: read by the engine, so on Linux beside the
/// system service's other files, and on a Mac in the account's ~/.kiri.
pub fn settings_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME") {
        return Path::new(&home).join(".kiri/registry-mirrors.json");
    }
    PathBuf::from("/etc/kiri/registry-mirrors.json")
}

pub fn load(path: &Path) -> Settings {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save(path: &Path, settings: &Settings) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(settings).map_err(std::io::Error::other)?)?;
    std::fs::rename(&tmp, path)
}

/// `docker.arvancloud.ir`, `https://mirror.example.com/`, `10.0.0.5:5000`
/// as the host a mirror list keeps; None for what isn't one.
pub fn host_of(mirror: &str) -> Option<String> {
    let s = mirror.trim();
    let s = s.strip_prefix("https://").or_else(|| s.strip_prefix("http://")).unwrap_or(s);
    let host = s.trim_end_matches('/').to_ascii_lowercase();
    crate::tls::valid_host(&host).then_some(host)
}

/// The `registry-mirrors` of Docker's own daemon.json, which Kiri honours
/// too.
fn docker_mirrors() -> Vec<String> {
    let mut paths = vec![PathBuf::from("/etc/docker/daemon.json")];
    if let Some(home) = std::env::var_os("HOME") {
        paths.push(Path::new(&home).join(".docker/daemon.json"));
    }
    paths
        .iter()
        .filter_map(|p| std::fs::read(p).ok())
        .filter_map(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .flat_map(|v| v["registry-mirrors"].as_array().cloned().unwrap_or_default())
        .filter_map(|m| m.as_str().and_then(host_of))
        .collect()
}

/// Every host to pull Hub images from, in the order to time them: one's
/// own mirrors, Docker's, the built-in ones, and Hub itself.
pub fn candidates(settings: &Settings, docker: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let own = settings.mirrors.iter().filter_map(|m| host_of(m));
    let built_in = BUILT_IN.iter().filter(|_| settings.builtin).map(|m| (*m).to_owned());
    for h in own.chain(docker.iter().cloned()).chain(built_in).chain(std::iter::once(ORIGIN.to_owned())) {
        if !out.contains(&h) {
            out.push(h);
        }
    }
    out
}

/// This machine's candidates, from its settings and Docker's.
pub fn configured() -> Vec<String> {
    candidates(&load(&settings_path()), &docker_mirrors())
}

/// The candidates put in order by how fast each answered (None: it didn't);
/// those that didn't answer go last, Hub itself last of them, so a pull
/// still has something to try.
pub fn order(timed: &[(String, Option<Duration>)]) -> Vec<String> {
    let mut answered: Vec<&(String, Option<Duration>)> = timed.iter().filter(|(_, t)| t.is_some()).collect();
    answered.sort_by_key(|(_, t)| *t);
    let mut silent: Vec<&String> = timed.iter().filter(|(_, t)| t.is_none()).map(|(h, _)| h).collect();
    silent.sort_by_key(|h| *h == ORIGIN);
    answered.into_iter().map(|(h, _)| h.clone()).chain(silent.into_iter().cloned()).collect()
}

struct Ranking {
    at: Instant,
    candidates: Vec<String>,
    order: Vec<String>,
}

static RANKING: Mutex<Option<Ranking>> = Mutex::new(None);

/// The ranking for `candidates` made in the last ten minutes, if any.
pub fn cached(candidates: &[String]) -> Option<Vec<String>> {
    let slot = RANKING.lock().unwrap_or_else(|e| e.into_inner());
    slot.as_ref().filter(|r| r.candidates == candidates && r.at.elapsed() < RANK_FOR).map(|r| r.order.clone())
}

pub fn remember(candidates: Vec<String>, order: Vec<String>) {
    *RANKING.lock().unwrap_or_else(|e| e.into_inner()) = Some(Ranking { at: Instant::now(), candidates, order });
}

/// A host that just failed a pull goes to the back of the ranking, so the
/// next pull starts with one that works.
pub fn failed(host: &str) {
    if let Some(r) = RANKING.lock().unwrap_or_else(|e| e.into_inner()).as_mut()
        && let Some(i) = r.order.iter().position(|h| h == host)
    {
        let h = r.order.remove(i);
        r.order.push(h);
    }
}

/// The order Kiri's builder should try Hub's mirrors in: the last ranking,
/// or the configured order before any; Hub itself is BuildKit's own last
/// resort, so it is left out.
pub fn for_builder() -> Vec<String> {
    let candidates = configured();
    cached(&candidates).unwrap_or(candidates).into_iter().filter(|h| h != ORIGIN).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn candidates_are_ones_own_then_dockers_then_built_in_then_hub() {
        let mine = Settings { mirrors: s(&["https://Mine.example.com/", "focker.ir", "not a host"]), builtin: true };
        let c = candidates(&mine, &s(&["docker.example.org"]));
        assert_eq!(&c[..3], &s(&["mine.example.com", "focker.ir", "docker.example.org"])[..]);
        assert_eq!(c.iter().filter(|h| *h == "focker.ir").count(), 1);
        assert_eq!(c.last().map(String::as_str), Some(ORIGIN));
        assert_eq!(c.len(), 3 + BUILT_IN.len() - 1 + 1);
        let off = Settings { mirrors: s(&["mine.example.com"]), builtin: false };
        assert_eq!(candidates(&off, &[]), s(&["mine.example.com", ORIGIN]));
    }

    #[test]
    fn the_fastest_answer_goes_first_and_silence_last() {
        let ms = |n| Some(Duration::from_millis(n));
        let timed = vec![(ORIGIN.to_owned(), None), ("slow".to_owned(), ms(900)), ("dead".to_owned(), None), ("fast".to_owned(), ms(40))];
        assert_eq!(order(&timed), s(&["fast", "slow", "dead", ORIGIN]));
    }

    #[test]
    fn a_failed_host_goes_to_the_back() {
        let c = s(&["a", "b", ORIGIN]);
        remember(c.clone(), c.clone());
        failed("a");
        assert_eq!(cached(&c), Some(s(&["b", ORIGIN, "a"])));
        assert_eq!(cached(&s(&["other"])), None);
    }

    #[test]
    fn settings_round_trip_and_default_to_the_built_in_list() {
        let path = std::env::temp_dir().join(format!("kiri-mirrors-{}/registry-mirrors.json", std::process::id()));
        assert_eq!(load(&path), Settings::default());
        let mine = Settings { mirrors: s(&["mine.example.com"]), builtin: false };
        save(&path, &mine).unwrap();
        assert_eq!(load(&path), mine);
        std::fs::write(&path, r#"{"mirrors":["x.example.com"]}"#).unwrap();
        assert!(load(&path).builtin);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn mirror_addresses_become_hosts() {
        assert_eq!(host_of("https://docker.arvancloud.ir/").as_deref(), Some("docker.arvancloud.ir"));
        assert_eq!(host_of("10.0.0.5:5000").as_deref(), Some("10.0.0.5:5000"));
        assert_eq!(host_of("https://x.example.com/path"), None);
        assert_eq!(host_of(""), None);
    }
}
