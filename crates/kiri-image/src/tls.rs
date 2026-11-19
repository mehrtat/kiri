//! What a registry connection trusts, decided as Docker decides it: the
//! system's certificate store (where a company's CA goes, with
//! update-ca-certificates or into the Keychain), and for one registry the
//! files in `certs.d/<host>/`: `*.crt` CAs, and a `*.cert` with its `*.key`
//! to sign in with a client certificate. Built-in roots alone turned away
//! every registry whose certificate a private CA signed.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::SystemTime;

use ureq::tls::{Certificate, ClientCert, PemItem, PrivateKey, RootCerts, TlsConfig};

/// The system's trusted roots, read once.
static SYSTEM: LazyLock<Vec<Certificate<'static>>> = LazyLock::new(|| {
    let found = rustls_native_certs::load_native_certs();
    if found.certs.is_empty() {
        eprintln!("kiri: no CA certificates in the system store; using the built-in roots");
    }
    found.certs.iter().map(|c| Certificate::from_der(c.as_ref()).to_owned()).collect()
});

/// A `certs.d` file, with what tells a changed one apart.
pub(crate) type HostFile = (PathBuf, u64, Option<SystemTime>);

/// The TLS settings for a registry without files of its own.
pub(crate) fn default_config() -> TlsConfig {
    TlsConfig::builder().root_certs(roots(&SYSTEM, &[])).build()
}

/// The system's roots and `extra`; the built-in Mozilla roots on a system
/// with no store and nothing extra, so such a system still pulls from
/// public registries.
fn roots(system: &[Certificate<'static>], extra: &[Certificate<'static>]) -> RootCerts {
    if system.is_empty() && extra.is_empty() {
        return RootCerts::WebPki;
    }
    RootCerts::new_with_certs(&[system, extra].concat())
}

/// Where Docker looks for a registry's certificates: the engine's directory,
/// and the one in the home directory that Docker Desktop reads.
pub fn cert_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/etc/docker/certs.d")];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(Path::new(&home).join(".docker/certs.d"));
    }
    dirs
}

/// `host`'s certificate files (`host` as Docker names the directory:
/// `registry.example.com`, `10.0.0.5:5000`) in each of `dirs`, in name
/// order. Empty when it has none.
pub(crate) fn host_files(dirs: &[PathBuf], host: &str) -> Vec<HostFile> {
    // The host comes from a URL; it never names another directory.
    if host.is_empty() || host.starts_with('.') || host.contains(['/', '\\']) {
        return Vec::new();
    }
    let mut files = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir.join(host)) else { continue };
        let mut found: Vec<HostFile> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "crt" || x == "cert" || x == "key"))
            .filter_map(|p| {
                let meta = std::fs::metadata(&p).ok().filter(std::fs::Metadata::is_file)?;
                Some((p, meta.len(), meta.modified().ok()))
            })
            .collect();
        found.sort();
        files.extend(found);
    }
    files
}

/// The TLS settings for a registry with `files`: their CAs trusted beside
/// the system's, and the client certificate a `.cert` and the `.key` of the
/// same name make. A `.cert` without its key is an error, as on Docker.
pub(crate) fn host_config(files: &[HostFile]) -> Result<TlsConfig, String> {
    let mut cas = Vec::new();
    let mut client = None;
    for (path, _, _) in files {
        let pem = || std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()));
        match path.extension().and_then(|x| x.to_str()) {
            Some("crt") => {
                let found = certificates(&pem()?);
                if found.is_empty() {
                    return Err(format!("{}: no certificate in it", path.display()));
                }
                cas.extend(found);
            }
            Some("cert") => {
                let key_path = path.with_extension("key");
                let key = std::fs::read(&key_path).map_err(|_| format!("missing key {} for client certificate {}", key_path.display(), path.display()))?;
                let key = PrivateKey::from_pem(&key).map_err(|e| format!("{}: {e}", key_path.display()))?;
                let chain = certificates(&pem()?);
                if chain.is_empty() {
                    return Err(format!("{}: no certificate in it", path.display()));
                }
                client = Some(ClientCert::new_with_certs(&chain, key));
            }
            _ => {}
        }
    }
    Ok(TlsConfig::builder().root_certs(roots(&SYSTEM, &cas)).client_cert(client).build())
}

/// Every certificate in a PEM file.
fn certificates(pem: &[u8]) -> Vec<Certificate<'static>> {
    ureq::tls::parse_pem(pem)
        .filter_map(|item| match item {
            Ok(PemItem::Certificate(c)) => Some(c),
            _ => None,
        })
        .collect()
}

/// Where `kiri registry trust` puts a registry's files: where the engine
/// reads them. On Linux the engine is a system service, which reads
/// `/etc/docker/certs.d` as dockerd does; on a Mac it runs as the user, as
/// Docker Desktop does with `~/.docker/certs.d`.
pub fn install_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME") {
        return Path::new(&home).join(".docker/certs.d");
    }
    PathBuf::from("/etc/docker/certs.d")
}

/// Whether `host` (`registry.example.com`, `10.0.0.5:5000`) can name a
/// `certs.d` directory.
pub fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && !host.starts_with(['.', '-'])
        && host.len() <= 255
        && host.bytes().all(|b| b.is_ascii_alphanumeric() || b"-.:[]".contains(&b))
        && !host.contains("..")
}

/// Writes `host`'s CA (`ca.crt`) and client certificate (`client.cert`,
/// `client.key`, only the owner may read the key) under `dir`, after
/// checking each is what it says. Returns the host's directory.
pub fn install(dir: &Path, host: &str, ca: Option<&[u8]>, client: Option<(&[u8], &[u8])>) -> Result<PathBuf, String> {
    if !valid_host(host) {
        return Err(format!("not a registry host: {host:?}"));
    }
    if ca.is_none() && client.is_none() {
        return Err("nothing to install: no CA and no client certificate".into());
    }
    if let Some(ca) = ca
        && certificates(ca).is_empty()
    {
        return Err("the CA file has no PEM certificate in it".into());
    }
    if let Some((cert, key)) = client {
        if certificates(cert).is_empty() {
            return Err("the client certificate file has no PEM certificate in it".into());
        }
        PrivateKey::from_pem(key).map_err(|e| format!("the client key file: {e}"))?;
    }
    let target = dir.join(host);
    let denied = |e: std::io::Error| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            format!("{}: permission denied (run it with sudo)", target.display())
        } else {
            format!("{}: {e}", target.display())
        }
    };
    std::fs::create_dir_all(&target).map_err(denied)?;
    if let Some(ca) = ca {
        std::fs::write(target.join("ca.crt"), ca).map_err(denied)?;
    }
    if let Some((cert, key)) = client {
        std::fs::write(target.join("client.cert"), cert).map_err(denied)?;
        write_private(&target.join("client.key"), key).map_err(denied)?;
    }
    Ok(target)
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    f.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

/// The registries with files of their own in `dirs`, and those files.
pub fn hosts(dirs: &[PathBuf]) -> Vec<(String, Vec<PathBuf>)> {
    let mut out: Vec<(String, Vec<PathBuf>)> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut names: Vec<String> = entries.flatten().filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().into_string().ok()).collect();
        names.sort();
        for name in names {
            let files: Vec<PathBuf> = host_files(std::slice::from_ref(dir), &name).into_iter().map(|(p, ..)| p).collect();
            if !files.is_empty() {
                out.push((name, files));
            }
        }
    }
    out
}

/// Removes what `install` put under `dir` for `host`; false when there was
/// nothing.
pub fn remove(dir: &Path, host: &str) -> Result<bool, String> {
    if !valid_host(host) {
        return Err(format!("not a registry host: {host:?}"));
    }
    let target = dir.join(host);
    if !target.is_dir() {
        return Ok(false);
    }
    for name in ["ca.crt", "client.cert", "client.key"] {
        match std::fs::remove_file(target.join(name)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return Err(format!("{}: permission denied (run it with sudo)", target.display())),
            Err(e) => return Err(format!("{}: {e}", target.display())),
        }
    }
    // Only an emptied directory goes; files someone else put there stay.
    let _ = std::fs::remove_dir(&target);
    Ok(true)
}

/// The certificates `host` (`name` or `name:port`, 443 when none) sends in
/// its TLS handshake, its own first, in DER. Nothing is trusted or sent:
/// this is only to show them, so a person can decide to trust their CA.
pub fn presented_chain(host: &str) -> Result<Vec<Vec<u8>>, String> {
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use std::net::ToSocketAddrs;
    use std::sync::{Arc, Mutex};

    #[derive(Debug)]
    struct Record(Mutex<Vec<Vec<u8>>>, Arc<rustls::crypto::CryptoProvider>);
    impl ServerCertVerifier for Record {
        fn verify_server_cert(&self, leaf: &CertificateDer<'_>, rest: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
            *self.0.lock().unwrap_or_else(|e| e.into_inner()) = std::iter::once(leaf).chain(rest).map(|c| c.to_vec()).collect();
            Ok(ServerCertVerified::assertion())
        }
        fn verify_tls12_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &rustls::DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls12_signature(m, c, d, &self.1.signature_verification_algorithms)
        }
        fn verify_tls13_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &rustls::DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls13_signature(m, c, d, &self.1.signature_verification_algorithms)
        }
        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            self.1.signature_verification_algorithms.supported_schemes()
        }
    }

    if !valid_host(host) {
        return Err(format!("not a registry host: {host:?}"));
    }
    let (name, port) = split_host(host)?;
    let addr = host;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let record = Arc::new(Record(Mutex::new(Vec::new()), provider.clone()));
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(record.clone())
        .with_no_client_auth();
    let server_name = ServerName::try_from(name.to_owned()).map_err(|e| format!("{name}: {e}"))?;
    let mut conn = rustls::ClientConnection::new(Arc::new(config), server_name).map_err(|e| e.to_string())?;
    let target = (name, port).to_socket_addrs().map_err(|e| format!("{addr}: {e}"))?.next().ok_or_else(|| format!("{addr}: no address"))?;
    let mut sock = std::net::TcpStream::connect_timeout(&target, std::time::Duration::from_secs(10)).map_err(|e| format!("{addr}: {e}"))?;
    let _ = sock.set_read_timeout(Some(std::time::Duration::from_secs(10)));
    while conn.is_handshaking() {
        conn.complete_io(&mut sock).map_err(|e| format!("{addr}: TLS handshake: {e}"))?;
    }
    let chain = std::mem::take(&mut *record.0.lock().unwrap_or_else(|e| e.into_inner()));
    if chain.is_empty() {
        return Err(format!("{addr} sent no certificate"));
    }
    Ok(chain)
}

/// `registry.example.com` → (`registry.example.com`, 443); `10.0.0.5:5000`
/// → (`10.0.0.5`, 5000); `[::1]:5000` → (`::1`, 5000).
fn split_host(host: &str) -> Result<(&str, u16), String> {
    let port = |p: &str| p.parse::<u16>().map_err(|_| format!("not a port in {host:?}"));
    if let Some(rest) = host.strip_prefix('[') {
        let (ip, after) = rest.split_once(']').ok_or_else(|| format!("not a registry host: {host:?}"))?;
        return match after {
            "" => Ok((ip, 443)),
            p => Ok((ip, port(p.strip_prefix(':').ok_or_else(|| format!("not a registry host: {host:?}"))?)?)),
        };
    }
    match host.rsplit_once(':') {
        Some((name, p)) => Ok((name, port(p)?)),
        None => Ok((host, 443)),
    }
}

/// The certificate to trust for a registry that sent `chain`: the last one,
/// the CA nearest the top that it sends. A registry that sends only its own
/// certificate gives no CA to trust; its CA file is needed.
pub fn ca_of(chain: &[Vec<u8>]) -> Result<&[u8], String> {
    match chain {
        [_, .., top] => Ok(top),
        _ => Err("the registry sends only its own certificate, not its CA's: give the CA file with --ca".into()),
    }
}

/// `der` as a PEM certificate.
pub fn pem(der: &[u8]) -> String {
    let body = crate::b64::encode(der);
    let mut out = String::from("-----BEGIN CERTIFICATE-----\n");
    for line in body.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(line).unwrap_or_default());
        out.push('\n');
    }
    out.push_str("-----END CERTIFICATE-----\n");
    out
}

/// The SHA-256 fingerprint of a DER certificate, as `openssl x509
/// -fingerprint -sha256` prints it.
pub fn fingerprint(der: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(der).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// A TLS server on `127.0.0.1` with a certificate the test CA
/// (testdata/tls) signed, answering 200 to each of `connections` requests
/// whose handshake works; with `send_ca` it sends the CA after its own.
#[cfg(test)]
pub(crate) fn test_server(send_ca: bool, connections: usize) -> (String, std::thread::JoinHandle<()>) {
    use rustls::pki_types::pem::PemObject;
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let host = listener.local_addr().unwrap().to_string();
    let mut chain = rustls::pki_types::CertificateDer::pem_slice_iter(include_bytes!("../testdata/tls/server.crt")).collect::<Result<Vec<_>, _>>().unwrap();
    if send_ca {
        chain.extend(rustls::pki_types::CertificateDer::pem_slice_iter(include_bytes!("../testdata/tls/ca.crt")).map(Result::unwrap));
    }
    let key = rustls::pki_types::PrivateKeyDer::from_pem_slice(include_bytes!("../testdata/tls/server.key")).unwrap();
    let config = std::sync::Arc::new(
        rustls::ServerConfig::builder_with_provider(std::sync::Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .unwrap(),
    );
    let server = std::thread::spawn(move || {
        for stream in listener.incoming().take(connections) {
            let Ok(stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
            let mut tls = rustls::StreamOwned::new(rustls::ServerConnection::new(config.clone()).unwrap(), stream);
            let mut head = Vec::new();
            let mut buf = [0u8; 1024];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                match tls.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => head.extend_from_slice(&buf[..n]),
                }
            }
            if !head.is_empty() {
                let _ = tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                tls.conn.send_close_notify();
                let _ = tls.flush();
            }
        }
    });
    (host, server)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ca_a_registry_sends_is_the_one_to_trust() {
        let (host, server) = test_server(true, 1);
        let chain = presented_chain(&host).unwrap();
        let ca = certificates(include_bytes!("../testdata/tls/ca.crt"));
        assert_eq!(chain.len(), 2);
        assert_eq!(ca_of(&chain).unwrap(), ca[0].der());
        assert_eq!(certificates(pem(ca_of(&chain).unwrap()).as_bytes())[0].der(), ca[0].der());
        server.join().unwrap();

        let (host, server) = test_server(false, 1);
        let chain = presented_chain(&host).unwrap();
        assert!(ca_of(&chain).unwrap_err().contains("--ca"));
        server.join().unwrap();
    }

    #[test]
    fn hosts_split_into_name_and_port() {
        assert_eq!(split_host("registry.example.com"), Ok(("registry.example.com", 443)));
        assert_eq!(split_host("10.0.0.5:5000"), Ok(("10.0.0.5", 5000)));
        assert_eq!(split_host("[::1]:5000"), Ok(("::1", 5000)));
        assert_eq!(split_host("[::1]"), Ok(("::1", 443)));
        assert!(split_host("reg:http").is_err());
        assert!(!valid_host("../etc") && !valid_host("a/b") && !valid_host("") && !valid_host("-x"));
        assert!(valid_host("10.0.0.5:5000") && valid_host("[::1]:5000") && valid_host("reg.example.com"));
    }

    #[test]
    fn install_checks_writes_lists_and_removes() {
        let dir = temp("install");
        let ca = include_bytes!("../testdata/tls/ca.crt");
        let (cert, key) = (include_bytes!("../testdata/tls/server.crt"), include_bytes!("../testdata/tls/server.key"));
        assert!(install(&dir, "reg.example:5000", Some(b"junk"), None).unwrap_err().contains("no PEM certificate"));
        assert!(install(&dir, "reg.example:5000", None, Some((cert, b"junk"))).unwrap_err().contains("client key"));
        assert!(install(&dir, "../x", Some(ca), None).is_err());
        assert!(install(&dir, "reg.example:5000", None, None).is_err());
        let target = install(&dir, "reg.example:5000", Some(ca), Some((cert, key))).unwrap();
        assert_eq!(std::fs::read(target.join("ca.crt")).unwrap(), ca);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(target.join("client.key")).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let listed = hosts(std::slice::from_ref(&dir));
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].0, "reg.example:5000");
        assert_eq!(listed[0].1.len(), 3);
        assert!(host_config(&host_files(std::slice::from_ref(&dir), "reg.example:5000")).unwrap().client_cert().is_some());
        assert_eq!(remove(&dir, "reg.example:5000"), Ok(true));
        assert!(!target.exists());
        assert_eq!(remove(&dir, "reg.example:5000"), Ok(false));
        let _ = std::fs::remove_dir_all(dir);
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kiri-tls-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_hosts_files_are_its_certificates_and_keys_only() {
        let (a, b) = (temp("files-a"), temp("files-b"));
        std::fs::create_dir_all(a.join("reg.example:5000")).unwrap();
        std::fs::create_dir_all(b.join("reg.example:5000")).unwrap();
        for f in ["ca.crt", "client.cert", "client.key", "notes.txt"] {
            std::fs::write(a.join("reg.example:5000").join(f), "x").unwrap();
        }
        std::fs::write(b.join("reg.example:5000/other.crt"), "x").unwrap();
        let names: Vec<String> =
            host_files(&[a.clone(), b.clone()], "reg.example:5000").iter().map(|(p, ..)| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["ca.crt", "client.cert", "client.key", "other.crt"]);
        assert!(host_files(std::slice::from_ref(&a), "elsewhere.example").is_empty());
        for bad in ["", "..", "../reg.example:5000", "x/../reg.example:5000"] {
            assert!(host_files(&[a.join("sub")], bad).is_empty(), "{bad}");
        }
        let _ = std::fs::remove_dir_all(a);
        let _ = std::fs::remove_dir_all(b);
    }

    #[test]
    fn a_ca_file_is_trusted_beside_the_system() {
        let dir = temp("ca");
        std::fs::write(dir.join("ca.crt"), include_bytes!("../testdata/tls/ca.crt")).unwrap();
        let config = host_config(&host_files(std::slice::from_ref(&dir.parent().unwrap().to_path_buf()), dir.file_name().unwrap().to_str().unwrap())).unwrap();
        let RootCerts::Specific(certs) = config.root_certs() else { panic!("no specific roots") };
        assert_eq!(certs.len(), SYSTEM.len() + 1);
        assert_eq!(certs.last().unwrap().der(), certificates(include_bytes!("../testdata/tls/ca.crt"))[0].der());
        assert!(config.client_cert().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_client_certificate_needs_its_key() {
        let dir = temp("client");
        std::fs::write(dir.join("client.cert"), include_bytes!("../testdata/tls/server.crt")).unwrap();
        let files = || host_files(std::slice::from_ref(&dir.parent().unwrap().to_path_buf()), dir.file_name().unwrap().to_str().unwrap());
        assert!(host_config(&files()).unwrap_err().contains("missing key"));
        std::fs::write(dir.join("client.key"), include_bytes!("../testdata/tls/server.key")).unwrap();
        assert!(host_config(&files()).unwrap().client_cert().is_some());
        std::fs::write(dir.join("bad.crt"), "not a certificate").unwrap();
        assert!(host_config(&files()).unwrap_err().contains("no certificate"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn no_system_store_and_nothing_extra_keeps_the_built_in_roots() {
        assert!(matches!(roots(&[], &[]), RootCerts::WebPki));
        let ca = certificates(include_bytes!("../testdata/tls/ca.crt"));
        assert!(matches!(roots(&[], &ca), RootCerts::Specific(c) if c.len() == 1));
    }
}
