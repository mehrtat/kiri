//! Running Windows tools safely. `Command::new("reg")` (and friends) search
//! the directory of the running exe before System32, so a `reg.exe`,
//! `powershell.exe` or DLL planted next to kiri-win.exe — e.g. in the
//! unzipped bundle in Downloads — would run in their place. Every system
//! tool is resolved to its real System32 path, and the DLL search path is
//! pinned to System32 for anything loaded at run time (WebView2's loader).
//!
//! PowerShell only ever runs script files installed beside kiri-win.exe
//! (contrib/windows), with `-File` and their parameters as arguments; never
//! script text this binary carries, encoded or not. A hidden PowerShell fed
//! an encoded script is what malware droppers do, and Defender's machine
//! learning quarantined a kiri-win.exe that did it (Trojan:Win32/Bearfoos.A!ml).

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
use windows_sys::Win32::System::LibraryLoader::SetDefaultDllDirectories;
use windows_sys::Win32::System::SystemInformation::{GetSystemDirectoryW, IMAGE_FILE_MACHINE_AMD64, IMAGE_FILE_MACHINE_ARM64};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetExitCodeProcess, IsWow64Process2, WaitForSingleObject, INFINITE};
use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, SW_SHOWNORMAL, WM_NULL, WM_SETTINGCHANGE};

const LOAD_LIBRARY_SEARCH_SYSTEM32: u32 = 0x0000_0800;

/// What every script kiri-win runs gets before `-File <script>`: no
/// profile, no prompts, and no execution policy in the way of Kiri's own
/// unsigned scripts.
pub const PS_FILE: [&str; 5] = ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"];

/// `C:\Windows\System32`, from the API rather than any environment variable.
fn system32() -> PathBuf {
    let mut buf = [0u16; 260];
    // SAFETY: GetSystemDirectoryW fills `buf` with up to its length and
    // returns the count; 0 means it could not be read.
    let n = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if n == 0 || n > buf.len() {
        return PathBuf::from(r"C:\Windows\System32");
    }
    PathBuf::from(String::from_utf16_lossy(&buf[..n]))
}

/// The full path of a Windows tool. A name with a path separator (the
/// bundled docker CLI) is returned as given; a bare name resolves under
/// System32.
pub fn resolve(name: &str) -> PathBuf {
    if name.contains('\\') || name.contains('/') {
        return PathBuf::from(name);
    }
    let dir = system32();
    if name.eq_ignore_ascii_case("powershell.exe") || name.eq_ignore_ascii_case("powershell") {
        return dir.join("WindowsPowerShell").join("v1.0").join("powershell.exe");
    }
    let mut leaf = name.to_owned();
    if !leaf.to_ascii_lowercase().ends_with(".exe") {
        leaf.push_str(".exe");
    }
    let p = dir.join(&leaf);
    // wsl.exe is a System32 stub on Windows 10 2004+/11; fall back to PATH
    // only if it is somehow absent, so an old build still runs.
    if leaf.eq_ignore_ascii_case("wsl.exe") && !p.is_file() {
        return PathBuf::from("wsl.exe");
    }
    p
}

/// A `Command` for a Windows tool at its real path. Callers add their own
/// flags (e.g. no window).
pub fn tool(name: &str) -> Command {
    Command::new(resolve(name))
}

/// Runs a script file (contrib/windows/kiri-admin.ps1) in an elevated
/// PowerShell and waits for it: ShellExecuteEx's "runas", so Windows asks
/// for an administrator's approval first. Its exit code, or an error if it
/// did not start: ERROR_CANCELLED (1223) when the approval was declined.
/// Straight from here, with no PowerShell `Start-Process -Verb RunAs` in
/// between to carry it, and its console window shows (no
/// `-WindowStyle Hidden`): Windows' approval prompt does anyway.
pub fn elevated_script(script: &Path, args: &[&str]) -> io::Result<u32> {
    let script = script.to_string_lossy();
    let all: Vec<&str> = PS_FILE.iter().copied().chain(Some(&*script)).chain(args.iter().copied()).collect();
    elevated(&resolve("powershell.exe"), &all)
}

/// Runs a program elevated (ShellExecuteEx's "runas": Windows asks for an
/// administrator's approval first) and waits for it: its exit code, or
/// ERROR_CANCELLED (1223) when the approval was declined.
pub fn elevated(program: &Path, args: &[&str]) -> io::Result<u32> {
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let params = args.iter().copied().map(quote).collect::<Vec<_>>().join(" ");
    let (verb, file, params) = (wide("runas"), wide(&program.to_string_lossy()), wide(&params));
    // SAFETY: the strings outlive the call and are NUL-terminated; the
    // process handle ShellExecuteExW returns (NOCLOSEPROCESS) is waited on,
    // read and closed here. COM is initialized as ShellExecuteEx asks, and
    // uninitialized only if this call did it.
    unsafe {
        let com = CoInitializeEx(std::ptr::null(), (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32);
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.nShow = SW_SHOWNORMAL;
        let started = ShellExecuteExW(&mut info);
        let error = io::Error::last_os_error();
        if com >= 0 {
            CoUninitialize();
        }
        if started == 0 {
            return Err(error);
        }
        if info.hProcess == 0 {
            return Err(io::Error::other("Windows started nothing"));
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 0u32;
        let read = GetExitCodeProcess(info.hProcess, &mut code);
        let error = io::Error::last_os_error();
        CloseHandle(info.hProcess);
        if read == 0 {
            return Err(error);
        }
        Ok(code)
    }
}

/// One argument as Windows' command-line parsing (CommandLineToArgvW, and
/// PowerShell's) reads it back: quoted when it has a space, tab or quote,
/// with backslashes doubled only before a quote.
fn quote(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_owned();
    }
    let mut s = String::from('"');
    let mut slashes = 0;
    for c in arg.chars() {
        if c == '\\' {
            slashes += 1;
            continue;
        }
        s.extend(std::iter::repeat_n('\\', if c == '"' { slashes * 2 + 1 } else { slashes }));
        slashes = 0;
        s.push(c);
    }
    s.extend(std::iter::repeat_n('\\', slashes * 2));
    s.push('"');
    s
}

/// Tells running programs (Explorer, so new terminals) that the user's
/// environment variables changed, as [Environment]::SetEnvironmentVariable
/// does after it writes them.
pub fn environment_changed() {
    let what: Vec<u16> = "Environment".encode_utf16().chain(Some(0)).collect();
    let mut result = 0usize;
    // SAFETY: a broadcast with a NUL-terminated string that outlives the
    // call (SendMessageTimeout waits, at most 5 s per window).
    unsafe {
        SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, 0, what.as_ptr() as isize, SMTO_ABORTIFHUNG, 5000, &mut result);
    }
}

/// Whether a window of class `class` is open and answers within a second.
/// One whose process is being killed, or is hung, does not.
pub fn window_answers(class: *const u16) -> bool {
    // SAFETY: looking a window up by a NUL-terminated class name, then a
    // no-op message to it with a timeout.
    unsafe {
        let h = FindWindowW(class, std::ptr::null());
        if h == 0 {
            return false;
        }
        let mut result = 0usize;
        SendMessageTimeoutW(h, WM_NULL, 0, 0, SMTO_ABORTIFHUNG, 1000, &mut result) != 0
    }
}

/// Pins run-time DLL loading to System32, so a DLL dropped next to the exe is
/// not loaded in place of a system one. Call once, first thing in `main`.
/// (Statically imported DLLs are handled by the `/DEPENDENTLOADFLAG` and
/// static-CRT link flags in .cargo/config.toml.)
pub fn harden_dll_search() {
    // SAFETY: a process-wide setting with a constant flag.
    unsafe {
        SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32);
    }
}

/// This build's architecture as Windows names it: "x64" or "arm64".
pub const BUILD_ARCH: &str = if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" };

/// The PC's own processor, as Windows reports it even to a program running
/// under emulation (an x64 Kiri on Windows on Arm): "arm64", "x64", or
/// None when Windows does not say.
pub fn native_arch() -> Option<&'static str> {
    let (mut process, mut native) = (0u16, 0u16);
    // SAFETY: the current-process pseudo handle and two out-pointers to
    // locals; the call only writes them.
    if unsafe { IsWow64Process2(GetCurrentProcess(), &mut process, &mut native) } == 0 {
        return None;
    }
    match native {
        IMAGE_FILE_MACHINE_ARM64 => Some("arm64"),
        IMAGE_FILE_MACHINE_AMD64 => Some("x64"),
        _ => None,
    }
}

/// Why this Kiri cannot set up on this PC, if it cannot: its Linux half is
/// built for one architecture, and WSL2 runs the PC's own (an arm64
/// kernel on Windows on Arm, with no x86-64 emulation inside its distros),
/// and WSL's installer is per architecture too. An x64 Kiri does start on
/// an Arm PC, under Windows' emulation, so it says which to install.
pub fn arch_mismatch() -> Option<String> {
    mismatch(BUILD_ARCH, native_arch()?)
}

fn mismatch(build: &str, native: &str) -> Option<String> {
    (build != native).then(|| {
        let v = env!("CARGO_PKG_VERSION");
        format!("This is Kiri for {build} PCs, and this PC is {native}: its Linux cannot run Kiri's {build} engine. Install Kiri-{v}-{}.msi instead.", if native == "arm64" { "arm64" } else { "x86_64" })
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn quotes_arguments_as_windows_reads_them() {
        assert_eq!(super::quote("-Dns"), "-Dns");
        assert_eq!(super::quote(""), "\"\"");
        assert_eq!(super::quote(r"C:\Users\A B\kiri-admin.ps1"), r#""C:\Users\A B\kiri-admin.ps1""#);
        assert_eq!(super::quote(r"C:\A B\"), r#""C:\A B\\""#);
        assert_eq!(super::quote(r#"a "b""#), r#""a \"b\"""#);
        assert_eq!(super::quote(r#"a\"b"#), r#""a\\\"b""#);
    }

    #[test]
    fn names_the_installer_to_use() {
        assert_eq!(super::mismatch("x64", "x64"), None);
        assert_eq!(super::mismatch("arm64", "arm64"), None);
        let m = super::mismatch("x64", "arm64").unwrap();
        assert!(m.contains(&format!("Kiri-{}-arm64.msi", env!("CARGO_PKG_VERSION"))), "{m}");
        assert!(super::mismatch("arm64", "x64").unwrap().ends_with("-x86_64.msi instead."));
    }
}

/// The folder WSL keeps `distro`'s disk (ext4.vhdx) in: the BasePath of its
/// key under HKCU\Software\Microsoft\Windows\CurrentVersion\Lxss, read
/// rather than guessed (an imported distro lives wherever it was imported).
pub fn distro_dir(distro: &str) -> Option<PathBuf> {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY_CURRENT_USER, KEY_READ, RRF_RT_REG_SZ};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    // A REG_SZ value of `sub`, at most 1024 UTF-16 units.
    let value = |key: isize, sub: &[u16], name: &str| -> Option<String> {
        let name = wide(name);
        let mut buf = [0u16; 1024];
        let mut bytes = (buf.len() * 2) as u32;
        // SAFETY: an open key, NUL-terminated names and a buffer of `bytes` bytes.
        let r = unsafe { RegGetValueW(key, sub.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut bytes) };
        (r == ERROR_SUCCESS).then(|| String::from_utf16_lossy(&buf[..(bytes as usize / 2)]).trim_end_matches('\0').to_owned())
    };
    let path = wide(r"Software\Microsoft\Windows\CurrentVersion\Lxss");
    let mut key = 0isize;
    // SAFETY: opening a key by a NUL-terminated path into `key`.
    if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_READ, &mut key) } != ERROR_SUCCESS {
        return None;
    }
    let mut found = None;
    for i in 0.. {
        let mut sub = [0u16; 256];
        let mut len = sub.len() as u32;
        // SAFETY: the open key and a buffer of `len` units for the subkey's name.
        let r = unsafe { RegEnumKeyExW(key, i, sub.as_mut_ptr(), &mut len, std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
        if r != ERROR_SUCCESS {
            break;
        }
        let sub = &sub[..=len as usize];
        if value(key, sub, "DistributionName").is_some_and(|n| n.eq_ignore_ascii_case(distro)) {
            found = value(key, sub, "BasePath").map(|p| PathBuf::from(p.strip_prefix(r"\\?\").unwrap_or(&p)));
            break;
        }
    }
    // SAFETY: the key opened above, closed once.
    unsafe { RegCloseKey(key) };
    found
}

/// The free bytes on the drive that holds `path`, and its root (`C:\`).
pub fn free_bytes(path: &Path) -> Option<(String, u64)> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let at: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free = 0u64;
    // SAFETY: a NUL-terminated path; the totals we do not need are null.
    if unsafe { GetDiskFreeSpaceExW(at.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) } == 0 {
        return None;
    }
    let root = path.components().next().map(|c| c.as_os_str().to_string_lossy().into_owned()).unwrap_or_default();
    Some((root, free))
}

/// The drives Kiri Linux's disk may go on, each root (`D:\`) with its free
/// bytes: fixed and NTFS or ReFS (what WSL keeps a disk on), and not on USB
/// or an SD card, which Windows also calls fixed when it is a disk drive
/// and which can be unplugged. Never a network or removable drive.
pub fn fixed_drives() -> Vec<(String, u64)> {
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW};
    /// GetDriveTypeW's DRIVE_FIXED.
    const FIXED: u32 = 3;
    // SAFETY: no arguments; a bit per drive letter.
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u8 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i) as char;
        let root = format!("{letter}:\\");
        let wroot: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
        // SAFETY: a NUL-terminated root path.
        if unsafe { GetDriveTypeW(wroot.as_ptr()) } != FIXED {
            continue;
        }
        let mut fs = [0u16; 32];
        // SAFETY: the root, and a buffer of fs.len() units for the file system's name; the rest not asked for.
        let named = unsafe { GetVolumeInformationW(wroot.as_ptr(), std::ptr::null_mut(), 0, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), fs.as_mut_ptr(), fs.len() as u32) };
        let fs = String::from_utf16_lossy(&fs[..fs.iter().position(|&c| c == 0).unwrap_or(fs.len())]);
        if named == 0 || !matches!(fs.as_str(), "NTFS" | "ReFS") || unpluggable(letter) {
            continue;
        }
        if let Some((_, free)) = free_bytes(Path::new(&root)) {
            out.push((root, free));
        }
    }
    out
}

/// Whether the drive is on USB, an SD card or MMC; also when that cannot be
/// read, so only a drive known to be internal is used.
fn unpluggable(letter: char) -> bool {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{BusTypeMmc, BusTypeSd, BusTypeUsb, CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::{PropertyStandardQuery, StorageDeviceProperty, IOCTL_STORAGE_QUERY_PROPERTY, STORAGE_DEVICE_DESCRIPTOR, STORAGE_PROPERTY_QUERY};
    let path: Vec<u16> = format!("\\\\.\\{letter}:").encode_utf16().chain(Some(0)).collect();
    // SAFETY: opening the volume with no access rights, only to ask about it.
    let h = unsafe { CreateFileW(path.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, std::ptr::null(), OPEN_EXISTING, 0, 0) };
    if h == INVALID_HANDLE_VALUE {
        return true;
    }
    let query = STORAGE_PROPERTY_QUERY { PropertyId: StorageDeviceProperty, QueryType: PropertyStandardQuery, AdditionalParameters: [0] };
    // SAFETY: plain data; all-zero is valid before the call fills it.
    let mut d: STORAGE_DEVICE_DESCRIPTOR = unsafe { std::mem::zeroed() };
    let mut got = 0u32;
    // SAFETY: the handle opened above, the query and a descriptor of the sizes given.
    let ok = unsafe {
        DeviceIoControl(
            h,
            IOCTL_STORAGE_QUERY_PROPERTY,
            (&query as *const STORAGE_PROPERTY_QUERY).cast(),
            std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            (&mut d as *mut STORAGE_DEVICE_DESCRIPTOR).cast(),
            std::mem::size_of::<STORAGE_DEVICE_DESCRIPTOR>() as u32,
            &mut got,
            std::ptr::null_mut(),
        )
    };
    // SAFETY: the handle opened above, closed once.
    unsafe { CloseHandle(h) };
    ok == 0 || matches!(d.BusType, BusTypeUsb | BusTypeSd | BusTypeMmc)
}
