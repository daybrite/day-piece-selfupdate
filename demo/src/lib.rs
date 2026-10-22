//! Self-update demo. The Updates settings group, the Check for Updates… menu item, and the
//! check scheduler all come from day-piece-selfupdate's settings provider; this app only
//! installs Preferences. All app-owned UI text comes from generated resources.
use day::prelude::*;
use day_piece_selfupdate::{configuration, prepare, settings};
use day_piece_settings::prelude::*;

#[cfg(feature = "e2e")]
mod e2e;

day::resources!();
day::day_start!(options: window(), root);

pub fn window() -> day::WindowOptions {
    if fixture_command() {
        std::process::exit(0);
    }
    res::window_options().size(720.0, 500.0)
}

pub fn root() -> impl Piece {
    #[cfg(feature = "e2e")]
    e2e::start();
    // General (appearance, language) from Day; Updates arrives through the registered provider.
    let prefs = Preferences::new()
        .about(About::from_manifest().link(
            res::str::day_about_website(),
            "https://github.com/daybrite/day-piece-selfupdate",
        ))
        .group(general_group(res::locales::ALL))
        .install();
    let version = configuration()
        .map(|c| c.version)
        .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").into());
    column((
        label(res::str::app_title())
            .font(Font::Title)
            .id(day::id!(title)),
        label(res::str::version(version)).id(day::id!(version)),
        label(res::str::introduction()).id(day::id!(introduction)),
        // The same phase the Updates group shows, mirrored on the home page.
        label(move || settings::status_text(settings::phase().get()))
            .id(day::id!(home_update_status)),
        button(res::str::open_updates())
            .action(move || {
                prefs.open_at("updates");
            })
            .id(day::id!(open_updates)),
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
