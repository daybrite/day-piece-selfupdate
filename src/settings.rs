// Copyright © The Daybrite Project
// SPDX-License-Identifier: MPL-2.0
//! The Updates settings group: a `day-piece-settings` provider (docs/settings.md "Providers")
//! plus the policy it drives. An app that depends on this crate with the `settings` feature and
//! calls `Preferences::install()` gets the group, the Check for Updates… menu item, and a
//! scheduler; it writes no code of its own.
//!
//! Policy (the prefs in [`prefs`]): while `CHECK_AUTOMATICALLY` is on, a check runs once
//! `LAST_CHECK` + `INTERVAL` has elapsed (shortly after launch, then re-armed hourly). While
//! `AUTO_DOWNLOAD` is on, an available release is downloaded, verified, and staged in the
//! background, which leaves the group in [`Phase::Ready`]. Installation never happens on its
//! own: the staged update waits for the user's "Quit and install" (or a quit), and the helper
//! abandons it when the app keeps running past 90 seconds.
//!
//! Every string the group shows comes from this crate's private catalog
//! (`resource/locales/<lang>/app.ftl`), so a missing key in the app's catalog cannot break it.
use crate::{Phase, configuration, managed_installation, prepare};
use day_piece_settings::{Group, Pref, Provider, pref_enum, rows};
use day_reactive::{Scope, Signal, on_main};
use std::cell::Cell;
use std::time::{SystemTime, UNIX_EPOCH};

day_fluent::locales!();

/// The persisted policy, keyed under `day.updates.*`. Apps read them like any other pref.
pub mod prefs {
    use super::{Pref, pref_enum};

    pref_enum! {
        /// How often the scheduler checks while automatic checks are on.
        pub enum Interval { Daily = "daily", Weekly = "weekly", Monthly = "monthly" }
    }

    impl Interval {
        /// The interval in seconds (a month counts as 30 days).
        pub const fn seconds(self) -> i64 {
            match self {
                Interval::Daily => 24 * 60 * 60,
                Interval::Weekly => 7 * 24 * 60 * 60,
                Interval::Monthly => 30 * 24 * 60 * 60,
            }
        }
    }

    /// Whether the scheduler checks on its own.
    pub static CHECK_AUTOMATICALLY: Pref<bool> = Pref::new("day.updates.check", || true);
    /// How often it checks.
    pub static INTERVAL: Pref<Interval> = Pref::new("day.updates.interval", || Interval::Daily);
    /// Whether an available release is downloaded and staged without asking.
    pub static AUTO_DOWNLOAD: Pref<bool> = Pref::new("day.updates.auto_download", || true);
    /// Unix seconds of the last completed check; 0 = never.
    pub static LAST_CHECK: Pref<i64> = Pref::new("day.updates.last_check", || 0);
}

/// How long after launch the first scheduled check may run.
const STARTUP_DELAY_MS: u32 = 10_000;
/// How often the scheduler re-evaluates whether a check is due.
const TICK_MS: u32 = 60 * 60 * 1000;
/// How long a staged update stays [`Phase::Ready`] before the helper abandons it (the desktop
/// helpers and the macOS service all wait 90 seconds for the app to quit).
const READY_LAPSE_MS: u32 = 91_000;

day_piece_settings::provider! {
    /// The Updates group. Registered in the link-time slice; `Preferences::install` picks it up.
    pub static UPDATES: Provider = Provider {
        id: "updates",
        order: 900,
        available: supported_platform,
        group: updates_group,
        startup: Some(startup),
    };
}

/// The desktops this crate can replace an installation on. Elsewhere the group is absent; a
/// managed installation (Flatpak, Snap, MSIX) still shows the group, reduced to one note.
fn supported_platform() -> bool {
    cfg!(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "linux"
    ))
}

/// The provider's UI-thread state: the phase every row reads and the spinner flag.
#[derive(Clone, Copy)]
struct State {
    phase: Signal<Phase>,
    busy: Signal<bool>,
    /// Bumped by the scheduler so the relative "last checked" text ages while visible.
    clock: Signal<i64>,
}

thread_local! {
    static STATE: Cell<Option<State>> = const { Cell::new(None) };
    static SCHEDULED: Cell<bool> = const { Cell::new(false) };
}

fn state() -> State {
    STATE.with(|s| match s.get() {
        Some(st) => st,
        None => {
            let st = State {
                phase: Signal::global(Phase::Idle),
                busy: Signal::global(false),
                clock: Signal::global(now()),
            };
            s.set(Some(st));
            st
        }
    })
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The current phase of the updater, for an app that mirrors it elsewhere (a status item).
pub fn phase() -> Signal<Phase> {
    state().phase
}

// ---------------------------------------------------------------------------
// The group
// ---------------------------------------------------------------------------

fn updates_group() -> Group {
    let group = Group::new("updates", day_fluent::res::str::day_settings_updates())
        .icon(day_spec::Symbol::Refresh);
    if managed_installation() {
        return group.row(rows::note(res::str::managed()).id(day_core::id!(updates_managed)));
    }
    let st = state();
    use prefs::Interval;
    group
        .row(rows::toggle(
            &prefs::CHECK_AUTOMATICALLY,
            res::str::check_automatically(),
        ))
        .row(
            rows::choice(
                &prefs::INTERVAL,
                res::str::frequency(),
                [
                    (Interval::Daily, res::str::interval_daily()),
                    (Interval::Weekly, res::str::interval_weekly()),
                    (Interval::Monthly, res::str::interval_monthly()),
                ],
            )
            .enabled(prefs::CHECK_AUTOMATICALLY.signal()),
        )
        .row(rows::toggle(
            &prefs::AUTO_DOWNLOAD,
            res::str::auto_download(),
        ))
        .row(
            rows::info(res::str::last_checked(), move || last_checked_text(st))
                .id(day_core::id!(updates_last_checked)),
        )
        .row(
            rows::info(res::str::status(), move || status_text(st.phase.get()))
                .id(day_core::id!(updates_status)),
        )
        .row(
            rows::action(move || action_title(st.phase.get()), run_action)
                .busy(st.busy)
                .id(day_core::id!(updates_check_now)),
        )
}

/// The action row's title follows the phase: check, then download, then quit and install.
fn action_title(phase: Phase) -> String {
    match phase {
        Phase::Ready => res::str::quit_and_install().format(),
        Phase::Available => res::str::download_update().format(),
        _ => res::str::check_now().format(),
    }
}

/// What the action row does in each phase (see [`action_title`]).
fn run_action() {
    match state().phase.get_untracked() {
        Phase::Ready => day_core::quit(),
        Phase::Available => start(Work::Prepare),
        _ => run_check_now(),
    }
}

/// The relative "last checked" line: never, just now, or n minutes/hours/days ago. Reads the
/// clock signal so the text ages as the scheduler ticks.
fn last_checked_text(st: State) -> String {
    let last = prefs::LAST_CHECK.get();
    if last <= 0 {
        return res::str::never().format();
    }
    let elapsed = (st.clock.get().max(now()) - last).max(0);
    match elapsed {
        s if s < 60 => res::str::just_now().format(),
        s if s < 60 * 60 => res::str::minutes_ago(s / 60).format(),
        s if s < 24 * 60 * 60 => res::str::hours_ago(s / 3600).format(),
        s => res::str::days_ago(s / 86_400).format(),
    }
}

/// The localized status line for a phase.
pub fn status_text(phase: Phase) -> String {
    match phase {
        Phase::Idle => res::str::status_idle(),
        Phase::Checking => res::str::status_checking(),
        Phase::UpToDate => res::str::status_current(),
        Phase::Available => res::str::status_available(),
        Phase::Missing => res::str::status_missing(),
        Phase::Unconfigured => res::str::status_unconfigured(),
        Phase::Invalid => res::str::status_invalid(),
        Phase::Preparing => res::str::status_preparing(),
        Phase::Ready => res::str::status_ready(),
        Phase::Failed => res::str::status_failed(),
        Phase::Managed => res::str::managed(),
    }
    .format()
}

// ---------------------------------------------------------------------------
// Check and prepare, off the UI thread
// ---------------------------------------------------------------------------

/// What a background run does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Work {
    /// Discover and verify the latest release metadata; stop at [`Phase::Available`].
    Check,
    /// Check, then download and stage the release when one is available.
    CheckAndPrepare,
    /// Download and stage the release the last check found.
    Prepare,
}

/// The Check for Updates… command and the "Check now" row: a check that goes on to stage the
/// release when `AUTO_DOWNLOAD` is on.
pub fn run_check_now() {
    let work = if prefs::AUTO_DOWNLOAD.get_untracked() {
        Work::CheckAndPrepare
    } else {
        Work::Check
    };
    start(work);
}

/// Start `work` on a background thread unless a run is in flight or an update is already
/// staged. Every check records `LAST_CHECK`, found or not, so the "last checked" line always
/// names the latest one.
fn start(work: Work) {
    let st = state();
    if st.busy.get_untracked() || st.phase.get_untracked() == Phase::Ready {
        return;
    }
    st.busy.set(true);
    st.phase.set(if work == Work::Prepare {
        Phase::Preparing
    } else {
        Phase::Checking
    });
    std::thread::spawn(move || {
        let outcome = run_blocking(work);
        on_main(move || finish(work, outcome));
    });
}

/// Land a background run's result on the UI thread.
fn finish(work: Work, outcome: Result<Phase, String>) {
    let st = state();
    st.busy.set(false);
    if work != Work::Prepare {
        prefs::LAST_CHECK.set(now());
        st.clock.set(now());
    }
    let phase = match outcome {
        Ok(p) => p,
        Err(e) => {
            log::warn!("day-piece-selfupdate: {e}");
            if work == Work::Prepare {
                Phase::Failed
            } else {
                Phase::Invalid
            }
        }
    };
    st.phase.set(phase);
    if phase == Phase::Ready {
        // The helper gives up when the app does not quit in time; the release is still on
        // disk, so the row goes back to offering the download (which re-verifies it).
        day_core::task(async move {
            day_core::sleep(READY_LAPSE_MS).await;
            if st.phase.get_untracked() == Phase::Ready {
                st.phase.set(Phase::Available);
            }
        });
    }
}

fn run_blocking(work: Work) -> Result<Phase, String> {
    let c = configuration().map_err(|e| e.to_string())?;
    if c.key.is_empty() {
        return Ok(Phase::Unconfigured);
    }
    if managed_installation() {
        return Ok(Phase::Managed);
    }
    if work == Work::Prepare {
        return prepare_blocking(&c);
    }
    let available = if c.repository.is_empty() {
        // No feed configured: the inbox is the release (a local fixture or a sideloaded update).
        match crate::check(&c) {
            Ok(_) => true,
            Err(e) if is_not_found(e.as_ref()) => return Ok(Phase::Missing),
            Err(e) => return Err(e.to_string()),
        }
    } else {
        check_feed(&c)?
    };
    if !available {
        return Ok(Phase::UpToDate);
    }
    if work == Work::CheckAndPrepare {
        return prepare_blocking(&c);
    }
    Ok(Phase::Available)
}

fn is_not_found(e: &(dyn std::error::Error + Send + Sync + 'static)) -> bool {
    e.downcast_ref::<std::io::Error>()
        .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
}

/// Discover the latest release and cache its verified metadata in the inbox. `true` when it
/// is newer than this build.
#[cfg(feature = "github")]
fn check_feed(c: &crate::Configuration) -> Result<bool, String> {
    match crate::github::check(c).map_err(|e| e.to_string())? {
        Some(update) => {
            // Cache the authenticated metadata; the payload downloads only when preparing.
            crate::verification::atomic_write(&c.inbox.join("release.json"), &update.metadata)
                .map_err(|e| e.to_string())?;
            crate::verification::atomic_write(
                &c.inbox.join("release.sig"),
                update.signature.as_bytes(),
            )
            .map_err(|e| e.to_string())?;
            Ok(true)
        }
        None => Ok(false),
    }
}

#[cfg(not(feature = "github"))]
fn check_feed(_c: &crate::Configuration) -> Result<bool, String> {
    Err("release discovery needs the `github` feature".into())
}

/// Download (when a feed is configured), verify, and stage the cached release.
fn prepare_blocking(c: &crate::Configuration) -> Result<Phase, String> {
    #[cfg(feature = "github")]
    if !c.repository.is_empty() {
        let update = crate::github::Update {
            release: crate::check(c).map_err(|e| e.to_string())?,
            metadata: std::fs::read(c.inbox.join("release.json")).map_err(|e| e.to_string())?,
            signature: std::fs::read_to_string(c.inbox.join("release.sig"))
                .map_err(|e| e.to_string())?,
        };
        crate::github::download_update(c, &update).map_err(|e| e.to_string())?;
    }
    match prepare(c).map_err(|e| e.to_string())?.as_str() {
        "ready" => Ok(Phase::Ready),
        "managed-installation" => Ok(Phase::Managed),
        other => Err(format!("installer answered {other:?}")),
    }
}

// ---------------------------------------------------------------------------
// Startup: the menu command and the scheduler
// ---------------------------------------------------------------------------

/// Runs once inside `Preferences::install`: registers the Check for Updates… command and arms
/// the scheduler.
fn startup() {
    day_core::register_check_for_updates(run_check_now);
    if SCHEDULED.with(|s| s.replace(true)) {
        return;
    }
    // Turning automatic checks on runs an overdue check right away rather than at the next tick.
    Scope::root().enter(|| {
        prefs::CHECK_AUTOMATICALLY.watch(|on| {
            if *on {
                tick();
            }
        });
    });
    arm(STARTUP_DELAY_MS);
}

fn arm(ms: u32) {
    day_core::task(async move {
        day_core::sleep(ms).await;
        tick();
        arm(TICK_MS);
    });
}

/// Run a scheduled check when one is due. Scheduled runs stage the release only when
/// `AUTO_DOWNLOAD` says so; they never install.
fn tick() {
    let st = state();
    st.clock.set(now());
    if !prefs::CHECK_AUTOMATICALLY.get_untracked() {
        return;
    }
    if st.busy.get_untracked()
        || matches!(
            st.phase.get_untracked(),
            Phase::Ready | Phase::Checking | Phase::Preparing
        )
    {
        return;
    }
    let due = prefs::LAST_CHECK.get_untracked() + prefs::INTERVAL.get_untracked().seconds();
    if now() < due {
        return;
    }
    let work = if prefs::AUTO_DOWNLOAD.get_untracked() {
        Work::CheckAndPrepare
    } else {
        Work::Check
    };
    start(work);
}
