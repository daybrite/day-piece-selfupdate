//! Local signed-update prototype. All app-owned UI text comes from generated resources.
use day::prelude::*;
use day_piece_selfupdate::{
    Phase, check, configuration, prepare,
    ui::{Labels, update_panel},
};

#[cfg(feature = "e2e")]
mod e2e;

day::resources!();
day::day_start!(options: window(), root);

pub fn window() -> day::WindowOptions {
    if fixture_command() {
        std::process::exit(0);
    }
    day::WindowOptions {
        locales: Some((res::locales::DEFAULT, res::locales::CATALOG)),
        title_fn: Some(|| res::str::app_title().format()),
        size: Size::new(720.0, 500.0),
        ..Default::default()
    }
}

fn status(phase: Phase) -> String {
    match phase {
        Phase::Checking => res::str::status_checking().format(),
        Phase::UpToDate => res::str::status_current().format(),
        Phase::Managed => res::str::status_managed().format(),
        Phase::Idle => res::str::status_idle().format(),
        Phase::Available => res::str::status_available().format(),
        Phase::Missing => res::str::status_missing().format(),
        Phase::Unconfigured => res::str::status_unconfigured().format(),
        Phase::Invalid => res::str::status_invalid().format(),
        Phase::Preparing => res::str::status_preparing().format(),
        Phase::Ready => res::str::status_ready().format(),
        Phase::Failed => res::str::status_failed().format(),
    }
}

pub fn root() -> impl Piece {
    #[cfg(feature = "e2e")]
    e2e::start();
    let phase = Signal::new(Phase::Idle);
    let version = configuration()
        .map(|c| c.version)
        .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").into());
    column((
        label(res::str::app_title()).font(Font::Title).id("title"),
        label(res::str::version(version)).id("version"),
        label(res::str::introduction()).id("introduction"),
        update_panel(
            phase,
            Labels {
                check: res::str::check_update().format(),
                install: res::str::prepare_update().format(),
                quit: res::str::quit_update().format(),
            },
            status,
            move || {
                phase.set(Phase::Checking);
                let setter = phase.setter();
                std::thread::spawn(move || {
                    let result = (|| -> day_piece_selfupdate::verification::Result<Phase> {
                        let c = configuration()?;
                        if c.key.is_empty() {
                            return Ok(Phase::Unconfigured);
                        }
                        if day_piece_selfupdate::managed_installation() {
                            return Ok(Phase::Managed);
                        }
                        if c.repository.is_empty() {
                            check(&c)?;
                            return Ok(Phase::Available);
                        }
                        match day_piece_selfupdate::github::check(&c)? {
                            Some(update) => {
                                // Cache authenticated metadata; payload downloads occur only on Prepare.
                                day_piece_selfupdate::verification::atomic_write(
                                    &c.inbox.join("release.json"),
                                    &update.metadata,
                                )?;
                                day_piece_selfupdate::verification::atomic_write(
                                    &c.inbox.join("release.sig"),
                                    update.signature.as_bytes(),
                                )?;
                                Ok(Phase::Available)
                            }
                            None => Ok(Phase::UpToDate),
                        }
                    })();
                    setter.set(match result {
                        Ok(p) => p,
                        Err(e) => {
                            eprintln!("check: {e}");
                            Phase::Invalid
                        }
                    });
                });
            },
            move || {
                phase.set(Phase::Preparing);
                let setter = phase.setter();
                std::thread::spawn(move || {
                    let result = configuration().and_then(|c| {
                        if !c.repository.is_empty() {
                            let update = day_piece_selfupdate::github::Update {
                                release: check(&c)?,
                                metadata: std::fs::read(c.inbox.join("release.json"))?,
                                signature: std::fs::read_to_string(c.inbox.join("release.sig"))?,
                            };
                            day_piece_selfupdate::github::download_update(&c, &update)?;
                        }
                        prepare(&c)
                    });
                    match result {
                        Ok(s) if s == "ready" => {
                            setter.set(Phase::Ready);
                            // The prototype helper cancels if the app does not exit within 90s.
                            std::thread::sleep(std::time::Duration::from_secs(91));
                            setter.set(Phase::Failed);
                        }
                        Ok(s) if s == "managed-installation" => setter.set(Phase::Managed),
                        other => {
                            eprintln!("prepare: {other:?}");
                            setter.set(Phase::Failed);
                        }
                    }
                });
            },
            day::quit,
        ),
    ))
    .spacing(18.0)
    .padding(24.0)
}

/// Fixture driver calls the same native client as the visible UI.
pub fn fixture_command() -> bool {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|s| s == "--selfupdate-probe") {
        match configuration() {
            Ok(c) => {
                let probe = c
                    .bundle
                    .parent()
                    .unwrap()
                    .join(format!(".sandbox-probe-{}", std::process::id()));
                let writable = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&probe)
                    .is_ok();
                if writable {
                    let _ = std::fs::remove_file(probe);
                }
                let mut value = serde_json::to_value(&c).unwrap();
                value["can_write_installation_parent"] = writable.into();
                println!("{value}");
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        return true;
    }
    if args.iter().any(|s| s == "--selfupdate-install") {
        let result = configuration().and_then(|c| prepare(&c));
        match result {
            Ok(s) if s == "ready" => println!("ready"),
            other => {
                eprintln!("{other:?}");
                std::process::exit(1);
            }
        }
        return true;
    }
    if args.iter().any(|s| s == "--selfupdate-relaunched")
        && let Ok(c) = configuration()
    {
        let _ = std::fs::write(
            c.inbox.join("relaunched.json"),
            serde_json::to_vec(&c).unwrap(),
        );
    }
    false
}
