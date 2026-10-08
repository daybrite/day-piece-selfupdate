#[cfg(not(target_os = "macos"))]
use crate::{
    Configuration,
    verification::{self, Result, desktop},
};

#[cfg(not(target_os = "macos"))]
pub fn configuration() -> Result<Configuration> {
    let executable = std::env::current_exe()?;
    let bundle = if cfg!(target_os = "linux") {
        std::env::var_os("APPIMAGE")
            .map(std::path::PathBuf::from)
            .unwrap_or(executable)
    } else {
        executable
    };
    let inbox = std::env::temp_dir().join("day-selfupdate-dev.daybrite.selfupdatedemo");
    desktop::private_directory(&inbox)?;
    Ok(Configuration {
        application_id: "dev.daybrite.selfupdatedemo".into(),
        build: option_env!("DAY_UPDATE_BUILD").unwrap_or("1").into(),
        version: option_env!("DAY_UPDATE_VERSION")
            .unwrap_or(env!("CARGO_PKG_VERSION"))
            .into(),
        key: option_env!("DAY_UPDATE_PUBLIC_KEY").unwrap_or("").into(),
        target: option_env!("DAY_UPDATE_TARGET").unwrap_or("").into(),
        repository: option_env!("DAY_UPDATE_REPOSITORY").unwrap_or("").into(),
        inbox,
        bundle,
        development: cfg!(debug_assertions),
    })
}

/// Detect managed package environments before preparing any executable replacement.
pub fn managed_installation() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::path::Path::new("/.flatpak-info").exists() || std::env::var_os("SNAP").is_some()
    }
    #[cfg(windows)]
    {
        let mut length = 0;
        // SAFETY: a null buffer and zero length queries package identity without writing a buffer.
        (unsafe {
            windows_sys::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName(
                &mut length,
                std::ptr::null_mut(),
            )
        }) != 15700
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        false
    }
}

#[cfg(not(target_os = "macos"))]
pub fn prepare(config: &Configuration) -> Result<String> {
    if managed_installation() {
        return Ok("managed-installation".into());
    }
    #[cfg(target_os = "linux")]
    if std::env::var_os("APPIMAGE").is_none() {
        return Err("self-update requires an AppImage installation".into());
    }
    let release = crate::check(config)?;
    if release.target != config.target {
        return Err("wrong release platform".into());
    }
    let asset = release
        .helper
        .as_ref()
        .ok_or("release has no installer helper")?;
    let helper = config.inbox.join(&asset.name);
    desktop::verify_file(&helper, asset)?;
    verification::verify_archive(&config.inbox.join(&release.archive), &release)?;
    let destination = if cfg!(windows) {
        config
            .bundle
            .parent()
            .ok_or("missing install directory")?
            .to_path_buf()
    } else {
        config.bundle.clone()
    };
    let request = desktop::Request {
        application_id: config.application_id.clone(),
        current_build: config.build.parse()?,
        key: config.key.clone(),
        target: config.target.clone(),
        destination,
        executable: config.bundle.clone(),
        inbox: config.inbox.clone(),
        caller_pid: std::process::id(),
    };
    for name in ["ready", "outcome.json"] {
        let _ = std::fs::remove_file(config.inbox.join(name));
    }
    let path = config.inbox.join("request.json");
    verification::atomic_write(&path, &serde_json::to_vec(&request)?)?;
    let mut child = std::process::Command::new(helper)
        .arg("desktop-install")
        .arg(path)
        .spawn()?;
    let expected = child.id().to_string();
    let started = std::time::Instant::now();
    loop {
        if std::fs::read_to_string(config.inbox.join("ready")).is_ok_and(|s| s == expected) {
            return Ok("ready".into());
        }
        if child.try_wait()?.is_some() {
            return Err("installer rejected update; see outcome.json".into());
        }
        if started.elapsed().as_secs() > 45 {
            return Err("installer preparation timed out".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
