// Copyright © The Daybrite Project
// SPDX-License-Identifier: MPL-2.0
//! Experimental local signed-update client. No automatic network discovery or installation.
pub use day_selfupdate_core as verification;
#[cfg(feature = "github")]
pub mod github;
mod portable;
pub use portable::managed_installation;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
pub struct Configuration {
    pub application_id: String,
    pub build: String,
    pub version: String,
    pub key: String,
    pub inbox: PathBuf,
    pub bundle: PathBuf,
    pub development: bool,
    #[serde(default)]
    pub repository: String,
    #[serde(default)]
    pub target: String,
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn dysu_config() -> *mut std::ffi::c_char;
    fn dysu_begin(inbox: *const std::ffi::c_char) -> *mut std::ffi::c_char;
    fn dysu_free(value: *mut std::ffi::c_char);
}

#[cfg(target_os = "macos")]
fn native_string(value: *mut std::ffi::c_char) -> verification::Result<String> {
    if value.is_null() {
        return Err("native API returned null".into());
    }
    // SAFETY: native functions return an owned, NUL-terminated allocation.
    let result = unsafe { std::ffi::CStr::from_ptr(value) }
        .to_string_lossy()
        .into_owned();
    unsafe {
        dysu_free(value);
    }
    Ok(result)
}

pub fn configuration() -> verification::Result<Configuration> {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: Foundation API with no borrowed arguments.
        Ok(serde_json::from_str(&native_string(unsafe {
            dysu_config()
        })?)?)
    }
    #[cfg(not(target_os = "macos"))]
    {
        portable::configuration()
    }
}

pub fn check(config: &Configuration) -> verification::Result<verification::Release> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(config.inbox.join("release.json"))?
        .take(65537)
        .read_to_end(&mut bytes)?;
    let mut signature = String::new();
    std::fs::File::open(config.inbox.join("release.sig"))?
        .take(1025)
        .read_to_string(&mut signature)?;
    verification::verify(
        &bytes,
        &signature,
        &config.key,
        &config.application_id,
        config.build.parse()?,
    )
}

/// Runs off the UI thread. `ready` means the installer verified/staged the update and is
/// waiting for this process to quit. It does NOT mean the update has been installed.
pub fn prepare(config: &Configuration) -> verification::Result<String> {
    #[cfg(target_os = "macos")]
    {
        let inbox = std::ffi::CString::new(config.inbox.to_str().ok_or("non UTF-8 inbox")?)?;
        // SAFETY: CString remains valid through the blocking native call.
        native_string(unsafe { dysu_begin(inbox.as_ptr()) })
    }
    #[cfg(not(target_os = "macos"))]
    {
        portable::prepare(config)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Available,
    Missing,
    Unconfigured,
    Invalid,
    Checking,
    UpToDate,
    Managed,
    Preparing,
    Ready,
    Failed,
}

#[cfg(feature = "ui")]
pub mod ui {
    use super::Phase;
    use day_core::Piece;
    use day_pieces::prelude::*;
    use day_reactive::Signal;

    /// All labels come from the consuming application's generated localization accessors.
    pub struct Labels {
        pub check: String,
        pub install: String,
        pub quit: String,
    }

    /// A composite piece: native Day controls, no new platform renderer or hardcoded UI copy.
    pub fn update_panel(
        phase: Signal<Phase>,
        labels: Labels,
        status: impl Fn(Phase) -> String + 'static,
        check: impl Fn() + 'static,
        install: impl Fn() + 'static,
        quit: impl Fn() + 'static,
    ) -> impl Piece {
        column((
            label(move || status(phase.get())).id("update-status"),
            button(labels.check)
                .enabled(move || {
                    !matches!(
                        phase.get(),
                        Phase::Checking | Phase::Preparing | Phase::Ready
                    )
                })
                .action(check)
                .id("check-update"),
            button(labels.install)
                .enabled(move || phase.get() == Phase::Available)
                .action(install)
                .id("prepare-update"),
            button(labels.quit)
                .enabled(move || phase.get() == Phase::Ready)
                .action(quit)
                .id("quit-update"),
        ))
        .spacing(12.0)
    }
}
