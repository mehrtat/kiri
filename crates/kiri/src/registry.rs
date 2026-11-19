//! `kiri registry`: trusting a registry whose certificate a private CA
//! signed, and signing in to one with a client certificate. The files go
//! where Docker keeps them (`certs.d/<host>/`, kiri_image::tls), which the
//! engine reads on each request, so they count at once, with no restart.
//!
//! `kiri registry mirror`: Docker Hub's mirrors, which a pull of a Hub
//! image chooses between by how fast each answers (kiri_image::mirrors).

use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

use kiri_image::tls;

const USAGE: &str = "usage: kiri registry trust HOST [--ca FILE] [--client-cert FILE --client-key FILE] [--yes]\n\
    \x20      kiri registry ls\n\
    \x20      kiri registry rm HOST\n\
    \x20      kiri registry mirror [ls | test | add HOST... | rm HOST... | builtin on|off]\n\
    HOST is the registry as images name it: registry.example.com, 10.0.0.5:5000.\n\
    Without --ca or a client certificate, trust takes the CA the registry sends,\n\
    once its SHA-256 fingerprint is confirmed (--yes confirms it unasked).";

pub fn run(args: &[String]) -> io::Result<()> {
    match args.first().map(String::as_str) {
        Some("trust") => trust(&args[1..]),
        Some("mirror" | "mirrors") => mirror(&args[1..]),
        Some("ls" | "list") | None => list(),
        Some("rm" | "remove") => match args.get(1) {
            Some(host) => remove(host),
            None => Err(usage()),
        },
        _ => Err(usage()),
    }
}

fn usage() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, USAGE)
}

/// What `trust` was asked for.
#[derive(Debug, Default, PartialEq)]
struct Trust {
    host: String,
    ca: Option<PathBuf>,
    client_cert: Option<PathBuf>,
    client_key: Option<PathBuf>,
    yes: bool,
}

fn parse(args: &[String]) -> io::Result<Trust> {
    let mut t = Trust::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().map(|v| local_path(v)).ok_or_else(usage);
        match a.as_str() {
            "--ca" => t.ca = Some(value()?),
            "--client-cert" => t.client_cert = Some(value()?),
            "--client-key" => t.client_key = Some(value()?),
            "--yes" | "-y" => t.yes = true,
            h if !h.starts_with('-') && t.host.is_empty() => t.host = h.to_owned(),
            _ => return Err(usage()),
        }
    }
    if t.host.is_empty() || t.client_cert.is_some() != t.client_key.is_some() {
        return Err(usage());
    }
    Ok(t)
}

/// A Windows path (`C:\certs\ca.crt`) as WSL, where Windows' kiri runs this
/// command, reaches it; any other path as given.
fn local_path(p: &str) -> PathBuf {
    let b = p.as_bytes();
    if b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/') && Path::new("/mnt").is_dir() {
        return PathBuf::from(format!("/mnt/{}/{}", (b[0] as char).to_ascii_lowercase(), p[3..].replace('\\', "/")));
    }
    PathBuf::from(p)
}

fn read(path: &Path) -> io::Result<Vec<u8>> {
    std::fs::read(path).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))
}

fn trust(args: &[String]) -> io::Result<()> {
    let t = parse(args)?;
    if !tls::valid_host(&t.host) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("not a registry host: {:?}", t.host)));
    }
    let client = match (&t.client_cert, &t.client_key) {
        (Some(c), Some(k)) => Some((read(c)?, read(k)?)),
        _ => None,
    };
    let ca = match &t.ca {
        Some(path) => Some(read(path)?),
        // Asked for a client certificate only: the registry's own
        // certificate may well be one the system trusts already.
        None if client.is_some() => None,
        None => Some(fetched_ca(&t.host, t.yes)?),
    };
    let dir = tls::install(&tls::install_dir(), &t.host, ca.as_deref(), client.as_ref().map(|(c, k)| (&c[..], &k[..]))).map_err(io::Error::other)?;
    println!("Installed in {}", dir.display());
    match kiri_image::Client::new().reach(&t.host) {
        Ok(status) => println!("{} answers over https with its certificate trusted (HTTP {status})", t.host),
        Err(e) => eprintln!("kiri: {} could not be checked: {e}", t.host),
    }
    Ok(())
}

/// The CA `host` sends, as PEM, once its fingerprint is confirmed.
fn fetched_ca(host: &str, yes: bool) -> io::Result<Vec<u8>> {
    let chain = tls::presented_chain(host).map_err(io::Error::other)?;
    let ca = tls::ca_of(&chain).map_err(io::Error::other)?;
    println!("{host} sends a CA certificate with SHA-256 fingerprint\n  {}", tls::fingerprint(ca));
    if !yes {
        if !io::stdin().is_terminal() {
            return Err(io::Error::other("check the fingerprint, then confirm it with --yes"));
        }
        print!("Trust it for {host}? [y/N] ");
        io::stdout().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim(), "y" | "Y" | "yes") {
            return Err(io::Error::other("not trusted"));
        }
    }
    Ok(tls::pem(ca).into_bytes())
}

fn list() -> io::Result<()> {
    let hosts = tls::hosts(&tls::cert_dirs());
    if hosts.is_empty() {
        println!("No registry has certificates of its own ({}).", tls::install_dir().display());
    }
    for (host, files) in hosts {
        println!("{host}");
        for f in files {
            println!("  {}", f.display());
        }
    }
    Ok(())
}

fn remove(host: &str) -> io::Result<()> {
    if tls::remove(&tls::install_dir(), host).map_err(io::Error::other)? {
        println!("Removed {host}'s certificates");
    } else {
        println!("{host} had no certificates in {}", tls::install_dir().display());
    }
    Ok(())
}

/// `kiri registry mirror …`.
fn mirror(args: &[String]) -> io::Result<()> {
    use kiri_image::mirrors;
    let path = mirrors::settings_path();
    let edit = |f: &dyn Fn(&mut mirrors::Settings) -> io::Result<()>| -> io::Result<()> {
        let mut s = mirrors::load(&path);
        f(&mut s)?;
        mirrors::save(&path, &s).map_err(|e| match e.kind() {
            io::ErrorKind::PermissionDenied => io::Error::new(e.kind(), format!("{}: permission denied (run it with sudo)", path.display())),
            _ => io::Error::new(e.kind(), format!("{}: {e}", path.display())),
        })?;
        println!("Saved in {}; the next pull uses it.", path.display());
        Ok(())
    };
    let hosts = |rest: &[String]| -> io::Result<Vec<String>> {
        if rest.is_empty() {
            return Err(usage());
        }
        rest.iter().map(|h| mirrors::host_of(h).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("not a registry host: {h:?}")))).collect()
    };
    match args.first().map(String::as_str) {
        None | Some("ls" | "list") => show(false),
        Some("test") => show(true),
        Some("add") => {
            let new = hosts(&args[1..])?;
            edit(&|s| {
                for h in &new {
                    if !s.mirrors.contains(h) {
                        s.mirrors.push(h.clone());
                    }
                }
                Ok(())
            })
        }
        Some("rm" | "remove") => {
            let gone = hosts(&args[1..])?;
            edit(&|s| {
                s.mirrors.retain(|m| !gone.contains(m));
                Ok(())
            })
        }
        Some("builtin") => {
            let on = match args.get(1).map(String::as_str) {
                Some("on") => true,
                Some("off") => false,
                _ => return Err(usage()),
            };
            edit(&|s| {
                s.builtin = on;
                Ok(())
            })
        }
        _ => Err(usage()),
    }
}

/// The hosts a Hub pull tries, in the order it tries them, each with how
/// fast it answered; with `test`, also the digest each gives for
/// `alpine:latest`, beside the one most of them give.
fn show(test: bool) -> io::Result<()> {
    use kiri_image::mirrors;
    let client = kiri_image::Client::new();
    let settings = mirrors::load(&mirrors::settings_path());
    let candidates = mirrors::configured();
    let timed = client.time(&candidates);
    let order = mirrors::order(&timed);
    let digests: Vec<(String, Result<String, String>)> = if test {
        std::thread::scope(|s| {
            let runs: Vec<_> = order.iter().map(|h| s.spawn(|| (h.clone(), client.mirror_digest(h).map_err(|e| e.to_string())))).collect();
            runs.into_iter().filter_map(|r| r.join().ok()).collect()
        })
    } else {
        Vec::new()
    };
    let common = most_common(digests.iter().filter_map(|(_, d)| d.as_ref().ok()));
    println!("A Docker Hub pull tries, in this order:");
    for (i, host) in order.iter().enumerate() {
        let time = timed.iter().find(|(h, _)| h == host).and_then(|(_, t)| *t).map_or_else(|| "no answer".to_owned(), |t| format!("{} ms", t.as_millis()));
        let from = if host == mirrors::ORIGIN {
            "Docker Hub"
        } else if settings.mirrors.contains(host) {
            "yours"
        } else if mirrors::BUILT_IN.contains(&host.as_str()) {
            "built-in"
        } else {
            "daemon.json"
        };
        let mut line = format!("{:>3}. {host:<26} {time:>10}  {from}", i + 1);
        if let Some((_, d)) = digests.iter().find(|(h, _)| h == host) {
            line.push_str(&match d {
                Ok(d) if Some(d) == common.as_ref() => "  alpine:latest as most give it".to_owned(),
                Ok(d) => format!("  alpine:latest DIFFERS: {d}"),
                Err(e) => format!("  failed: {e}"),
            });
        }
        println!("{line}");
    }
    if !settings.builtin {
        println!("The built-in mirrors are off (kiri registry mirror builtin on).");
    }
    Ok(())
}

/// The value most of `items` share.
fn most_common<'a>(items: impl Iterator<Item = &'a String>) -> Option<String> {
    let mut counts: Vec<(&String, usize)> = Vec::new();
    for d in items {
        match counts.iter_mut().find(|(x, _)| *x == d) {
            Some((_, n)) => *n += 1,
            None => counts.push((d, 1)),
        }
    }
    counts.into_iter().max_by_key(|(_, n)| *n).map(|(d, _)| d.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn the_digest_most_give_is_the_common_one() {
        let v: Vec<String> = ["a", "b", "a", "c"].iter().map(|s| (*s).to_owned()).collect();
        assert_eq!(most_common(v.iter()).as_deref(), Some("a"));
        assert_eq!(most_common(std::iter::empty()), None);
    }

    #[test]
    fn trust_reads_its_flags() {
        assert_eq!(parse(&args("reg.example:5000 --yes")).unwrap(), Trust { host: "reg.example:5000".into(), yes: true, ..Default::default() });
        let t = parse(&args("reg.example --ca a.crt --client-cert c.cert --client-key c.key")).unwrap();
        assert_eq!((t.ca.unwrap(), t.client_cert.unwrap(), t.client_key.unwrap()), ("a.crt".into(), "c.cert".into(), "c.key".into()));
        for bad in ["", "--yes", "reg --ca", "reg --client-cert c.cert", "reg other", "reg --what"] {
            assert!(parse(&args(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_windows_path_is_reached_through_mnt() {
        let p = local_path(r"C:\certs\ca.crt");
        if Path::new("/mnt").is_dir() {
            assert_eq!(p, PathBuf::from("/mnt/c/certs/ca.crt"));
        }
        assert_eq!(local_path("/etc/ca.crt"), PathBuf::from("/etc/ca.crt"));
        assert_eq!(local_path("ca.crt"), PathBuf::from("ca.crt"));
    }
}
