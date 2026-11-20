//! What the engine's storage takes on disk, for `docker system df`: each
//! unpacked layer, each container's writable layer and each volume, as
//! `du -x` counts it (allocated blocks, hard links once, never across into
//! another filesystem such as a host folder or NFS export mounted on a
//! volume). Walking tens of gigabytes takes a while, so the agent runs this
//! in a child process of its own (`Measuring`), never on a thread: the agent
//! must stay single-threaded for clone3, and a container or exec cloned
//! while a thread held the allocator's lock waited on it forever, before
//! its execve.

use std::collections::HashSet;
use std::fs;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use super::sys;

/// A measurement under way in a child process, whose lines come back
/// through a pipe the agent polls.
pub struct Measuring {
    // The agent reaps it with every other child; the tests wait for it.
    #[cfg_attr(not(test), allow(dead_code))]
    pid: libc::pid_t,
    from: OwnedFd,
    got: Vec<u8>,
}

impl Measuring {
    /// Forks the child that runs `measure`. The caller must be
    /// single-threaded, as with any fork.
    pub fn start(layers: &Path, containers: &Path, volumes: &Path) -> io::Result<Self> {
        let (from, to) = sys::pipe()?;
        // SAFETY: the agent is single-threaded, so the child is a consistent
        // copy. It only walks the disk, writes to the pipe, then exits.
        match unsafe { libc::fork() } {
            0 => {
                // A walk can take minutes: none of the agent's descriptors
                // (a network's DNS socket) may stay open in it meanwhile.
                sys::close_now(&sys::close_ranges(&[to.as_raw_fd()]));
                let lines = measure(layers, containers, volumes);
                let mut out = fs::File::from(to);
                let code = i32::from(out.write_all(lines.join("\n").as_bytes()).is_err());
                // SAFETY: leaving the child without running the parent's destructors.
                unsafe { libc::_exit(code) }
            }
            pid if pid < 0 => Err(io::Error::last_os_error()),
            pid => {
                drop(to);
                sys::set_nonblocking(from.as_raw_fd())?;
                Ok(Measuring { pid, from, got: Vec::new() })
            }
        }
    }

    pub fn fd(&self) -> i32 {
        self.from.as_raw_fd()
    }

    #[cfg(test)]
    pub fn pid(&self) -> libc::pid_t {
        self.pid
    }

    /// Reads what the child wrote so far: the lines once it is done (the
    /// pipe closed), None while it still runs. The agent reaps the child
    /// like any other.
    pub fn read(&mut self) -> Option<Vec<String>> {
        let mut buf = [0u8; 16 << 10];
        loop {
            // SAFETY: reading into a buffer we own from a descriptor we own.
            let n = unsafe { libc::read(self.from.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if n > 0 {
                self.got.extend_from_slice(&buf[..n as usize]);
                continue;
            }
            match (n < 0).then(|| io::Error::last_os_error().kind()) {
                Some(io::ErrorKind::Interrupted) => {}
                Some(io::ErrorKind::WouldBlock) => return None,
                _ => break,
            }
        }
        Some(String::from_utf8_lossy(&self.got).lines().map(str::to_owned).collect())
    }
}

/// One `KIND NAME BYTES` line per entry (see `kiri_proto::Msg::Usage`).
pub fn measure(layers: &Path, containers: &Path, volumes: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    // Every entry in the layer store, half-done unpacks included, so the
    // total is what the store really takes.
    for (name, path) in entries(layers) {
        lines.push(format!("l {name} {}", du(&path)));
    }
    for (id, path) in entries(containers) {
        let upper = path.join("upper");
        if upper.is_dir() {
            lines.push(format!("c {id} {}", du(&upper)));
        }
    }
    for (name, path) in entries(volumes) {
        lines.push(format!("v {name} {}", du(&path)));
    }
    lines
}

/// The directories in `dir`, by a name that is safe on one line.
fn entries(dir: &Path) -> Vec<(String, std::path::PathBuf)> {
    let Ok(read) = fs::read_dir(dir) else { return Vec::new() };
    read.flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            (!name.is_empty() && !name.contains(char::is_whitespace)).then(|| (name, e.path()))
        })
        .collect()
}

/// Bytes allocated under `root`, on `root`'s filesystem only.
pub fn du(root: &Path) -> u64 {
    let Ok(top) = fs::symlink_metadata(root) else { return 0 };
    let dev = top.dev();
    let mut seen = HashSet::new();
    let mut total = top.blocks() * 512;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = fs::read_dir(&dir) else { continue };
        for entry in read.flatten() {
            let Ok(m) = entry.metadata() else { continue };
            if m.dev() != dev {
                continue;
            }
            if m.nlink() > 1 && !m.is_dir() && !seen.insert(m.ino()) {
                continue;
            }
            total += m.blocks() * 512;
            if m.is_dir() {
                stack.push(entry.path());
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_files_once_and_lists_each_kind() {
        let root = std::env::temp_dir().join(format!("kiri-usage-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (layers, containers, volumes) = (root.join("layers"), root.join("containers"), root.join("volumes"));
        fs::create_dir_all(layers.join("ab12")).unwrap();
        fs::create_dir_all(containers.join("c1/upper")).unwrap();
        fs::create_dir_all(volumes.join("data/_data/sub")).unwrap();
        fs::write(layers.join("ab12/f"), vec![1u8; 64 << 10]).unwrap();
        fs::write(volumes.join("data/_data/sub/big"), vec![1u8; 256 << 10]).unwrap();
        // A hard link costs nothing more; a symlink is not followed.
        fs::hard_link(volumes.join("data/_data/sub/big"), volumes.join("data/_data/again")).unwrap();
        std::os::unix::fs::symlink(&layers, volumes.join("data/_data/link")).unwrap();
        // A plain file in the store is not an entry.
        fs::write(layers.join("ab12.done"), b"").unwrap();

        let lines = measure(&layers, &containers, &volumes);
        let size = |kind: &str, name: &str| -> u64 {
            let l = lines.iter().find(|l| l.starts_with(&format!("{kind} {name} "))).unwrap_or_else(|| panic!("no {kind} {name} in {lines:?}"));
            l.rsplit(' ').next().unwrap().parse().unwrap()
        };
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(size("l", "ab12") >= 64 << 10);
        assert!(size("c", "c1") < 64 << 10);
        let v = size("v", "data");
        assert!((256 << 10..512 << 10).contains(&v), "the hard link was counted twice, or the symlink followed: {v}");

        // The agent's way: the same lines, from a child process.
        let mut m = Measuring::start(&layers, &containers, &volumes).unwrap();
        let mut status = 0;
        // SAFETY: waiting for the child we just forked.
        assert_eq!(unsafe { libc::waitpid(m.pid(), &mut status, 0) }, m.pid(), "the measurement is not a child process");
        assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0, "status {status}");
        assert_eq!(m.read(), Some(lines));
        let _ = fs::remove_dir_all(&root);
    }
}
