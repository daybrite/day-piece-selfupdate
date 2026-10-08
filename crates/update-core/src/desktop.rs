//! Unprivileged desktop installation. The helper is authenticated as an asset before launch.
use crate::{Result, Transaction, atomic_write, digest, remove_path, verify, verify_archive};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub application_id: String,
    pub current_build: u64,
    pub key: String,
    pub target: String,
    pub destination: PathBuf,
    pub executable: PathBuf,
    pub inbox: PathBuf,
    pub caller_pid: u32,
}

pub fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    if !fs::symlink_metadata(path)?.is_file() {
        return Err("expected regular file".into());
    }
    let mut bytes = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err("file exceeds limit".into());
    }
    Ok(bytes)
}

pub fn session_path(destination: &Path, id: &str) -> Result<PathBuf> {
    if !crate::safe_name(id) {
        return Err("invalid application identity".into());
    }
    Ok(destination
        .parent()
        .ok_or("destination has no parent")?
        .join(format!(".day-selfupdate-{id}")))
}

pub fn private_directory(path: &Path) -> Result<()> {
    #[allow(unused_mut)]
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e.into()),
    }
    let m = fs::symlink_metadata(path)?;
    if !m.is_dir() {
        return Err("unsafe session directory".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid takes no arguments.
        if m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
            return Err("session must be owned by this user and private".into());
        }
    }
    // On Windows this inherits the enclosing per-user directory's ACL. No elevation is used.
    Ok(())
}

pub fn install(request: &Request) -> Result<()> {
    if request.caller_pid <= 1 || request.caller_pid == std::process::id() {
        return Err("invalid caller PID".into());
    }
    if !request.destination.is_absolute() || !request.executable.is_absolute() {
        return Err("installation paths must be absolute".into());
    }
    let windows = request.target == format!("windows-winui-{}", std::env::consts::ARCH);
    let linux = request.target == format!("linux-gtk-{}", std::env::consts::ARCH);
    if !(windows && cfg!(windows) || linux && cfg!(target_os = "linux")) {
        return Err("helper platform or architecture mismatch".into());
    }
    if windows && request.executable.parent() != Some(request.destination.as_path()) {
        return Err("executable must be directly inside the installation directory".into());
    }
    if linux && request.executable != request.destination {
        return Err("AppImage executable and replacement path differ".into());
    }
    if fs::symlink_metadata(&request.destination)?
        .file_type()
        .is_symlink()
    {
        return Err("destination is a symlink".into());
    }
    let session = session_path(&request.destination, &request.application_id)?;
    private_directory(&session)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(session.join("lock"))?;
    lock.try_lock()?;
    for name in ["ready", "outcome.json"] {
        let _ = fs::remove_file(request.inbox.join(name));
    }
    let result = install_locked(request, &session, windows);
    if let Err(e) = &result {
        atomic_write(
            &request.inbox.join("outcome.json"),
            &serde_json::to_vec(
                &serde_json::json!({"state":"failed", "diagnostic":e.to_string()}),
            )?,
        )?;
    }
    result
}

fn install_locked(r: &Request, session: &Path, windows: bool) -> Result<()> {
    let bytes = read_limited(&r.inbox.join("release.json"), 65536)?;
    let signature = String::from_utf8(read_limited(&r.inbox.join("release.sig"), 1024)?)?;
    let release = verify(
        &bytes,
        &signature,
        &r.key,
        &r.application_id,
        r.current_build,
    )?;
    if release.schema != 2 || release.target != r.target {
        return Err("release target mismatch".into());
    }
    let staged = session.join(if windows {
        "installer.exe"
    } else {
        "next.appimage"
    });
    // Acquire a process handle before reporting readiness; Windows waits on the actual process,
    // not a reused PID. Linux uses a pidfd for the same reason.
    let caller = Caller::open(r.caller_pid)?;
    let transaction = Transaction {
        target: r.destination.clone(),
        stage: staged.clone(),
        backup: session.join("previous"),
        journal: session.join("journal"),
    };
    transaction.recover()?;
    // A new app explicitly asking for another update acknowledges the last committed install.
    if transaction.backup.exists() {
        let installed: u64 =
            String::from_utf8(read_limited(&session.join("installed-build"), 32)?)?.parse()?;
        if installed != r.current_build || fs::read(&transaction.journal)? != b"committed" {
            return Err("previous update needs recovery or confirmation".into());
        }
        remove_path(&transaction.backup)?;
    }
    fs::copy(r.inbox.join(&release.archive), &staged)?;
    verify_archive(&staged, &release)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o700))?;
    }
    atomic_write(
        &r.inbox.join("ready"),
        std::process::id().to_string().as_bytes(),
    )?;
    caller.wait()?;
    // Recheck the exact private payload immediately before executing/replacing it.
    verify_archive(&staged, &release)?;
    if windows {
        install_nsis(r, &staged, &transaction)?;
    } else {
        transaction.replace(|_| Ok(()))?;
    }
    atomic_write(
        &session.join("installed-build"),
        release.build.to_string().as_bytes(),
    )?;
    atomic_write(&r.inbox.join("outcome.json"), br#"{"state":"installed"}"#)?;
    let mut command = Command::new(&r.executable);
    command.arg("--selfupdate-relaunched");
    for name in [
        "APPIMAGE",
        "APPDIR",
        "ARGV0",
        "LD_LIBRARY_PATH",
        "LD_PRELOAD",
    ] {
        command.env_remove(name);
    }
    command.spawn()?;
    Ok(())
}

fn install_nsis(r: &Request, installer: &Path, t: &Transaction) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let destination = r.destination.to_str().ok_or("non UTF-8 install path")?;
        if destination.contains(['"', '\n', '\r']) {
            return Err("unsupported installation path".into());
        }
        // NSIS requires /D last and unquoted even when the directory contains spaces.
        atomic_write(&t.journal, b"prepared")?;
        fs::rename(&t.target, &t.backup)?;
        let result = (|| {
            let status = Command::new(installer)
                .arg("/S")
                .raw_arg(format!("/D={destination}"))
                .status()?;
            if !status.success() || !r.executable.is_file() {
                return Err("NSIS installation failed".into());
            }
            atomic_write(&t.journal, b"committed")
        })();
        if result.is_err() {
            if t.target.exists() {
                remove_path(&t.target)?;
            }
            fs::rename(&t.backup, &t.target)?;
            atomic_write(&t.journal, b"recovered")?;
        }
        result
    }
    #[cfg(not(windows))]
    {
        let _ = (r, installer, t);
        Err("NSIS requires Windows".into())
    }
}

struct Caller {
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
    #[cfg(target_os = "linux")]
    fd: std::os::fd::OwnedFd,
}
impl Caller {
    fn open(pid: u32) -> Result<Self> {
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE};
            // SAFETY: request a wait-only handle; owned by Caller until Drop.
            let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
            if handle.is_null() {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(Self { handle })
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::FromRawFd;
            // SAFETY: pidfd_open only obtains a handle; negative results are checked.
            let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as i32;
            if fd < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(Self {
                fd: unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) },
            })
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            let _ = pid;
            Err("unsupported helper platform".into())
        }
    }
    fn wait(&self) -> Result<()> {
        #[cfg(windows)]
        {
            // SAFETY: handle is live for this call.
            let result = unsafe {
                windows_sys::Win32::System::Threading::WaitForSingleObject(self.handle, 90_000)
            };
            if result != 0 {
                return Err("caller did not exit within 90 seconds".into());
            }
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            let mut fd = libc::pollfd {
                fd: self.fd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one live pollfd entry.
            if unsafe { libc::poll(&mut fd, 1, 90_000) } != 1 {
                return Err("caller did not exit within 90 seconds".into());
            }
        }
        Ok(())
    }
}
#[cfg(windows)]
impl Drop for Caller {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

pub fn verify_file(path: &Path, asset: &crate::Asset) -> Result<()> {
    let (size, sha256) = digest(path)?;
    if size != asset.size || sha256 != asset.sha256 {
        return Err("asset hash or size mismatch".into());
    }
    Ok(())
}
