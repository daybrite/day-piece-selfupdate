# day-piece-self-update

A native Day update panel and desktop self-update backends, with a standard `demo/` app and an official GitHub release workflow.

- **macOS:** sandboxed AppKit app, separately signed XPC launcher, verified whole-bundle replacement, and notarized release/update packages.
- **Windows:** verified per-user NSIS update, process-handle wait, backup/recovery, and relaunch.
- **Linux:** verified whole-AppImage replacement, pidfd wait, backup/recovery, and relaunch.
- **GitHub:** latest-stable discovery and tag-specific, signed metadata binding the package and installer-helper hashes to the app, target, version, and build.

The demo separates checking, downloading/preparing, and quitting to install. Every app-owned UI message uses generated localization accessors. Managed packages are detected before installation. Windows/Linux use a signed, downloaded Rust helper; macOS embeds its helper so a sandboxed client can invoke it through XPC. Helpers run as the user, never root.

## The Updates settings group

With the `settings` feature, the crate contributes an **Updates** group to any app that installs
[day-piece-settings](https://daybrite.dev/docs/settings) `Preferences`. The app adds nothing:
the provider registers itself at link time, and `Preferences::new().install()` picks it up.

| Row | Pref (key, default) |
| --- | --- |
| Check for updates automatically | `day.updates.check`, on |
| Check frequency: daily, weekly, monthly (enabled while the toggle is on) | `day.updates.interval`, daily |
| Download updates automatically | `day.updates.auto_download`, on |
| Last checked | `day.updates.last_check`, unix seconds, never |
| Status, and a button that reads Check now, Download update, or Quit and install as the phase moves | |

On a Flatpak, Snap, or MSIX installation the group reduces to one note saying the store or
package manager handles updates. The prefs are `day_piece_self_update::settings::prefs::*`;
`settings::phase()` is the live `Phase` for an app that mirrors it elsewhere.

The provider also registers the **Check for Updates…** menu item (`day::register_check_for_updates`),
which Day places under About in the macOS App menu and in Help on the other desktops. It runs
the same check as the button.

The scheduler starts with `Preferences::install`: about ten seconds after launch, then hourly,
it checks when automatic checks are on and the last check is older than the chosen interval.
When background downloads are on and a newer release is found, it downloads, verifies, and
stages it, and the group shows **Quit and install**. Nothing installs until the user quits; the
helper abandons a staged update the app keeps running past for 90 seconds, and the group goes
back to offering the download. The group's strings ship in the crate's own Fluent catalog
(`resource/locales/`, seven languages), so an app's catalog cannot leave a key missing.

Start with [signing and release setup](docs/RELEASING.md). The private update key and the App Fair Developer ID/notary credentials must be provisioned before an official release can pass. The existing broader roadmap is [DAY_UPDATER_PLAN.md](../DAY_UPDATER_PLAN.md).

## Local development

Dependencies use Day's canonical Git URL. With an adjacent Day checkout, configure ignored local patches:

```sh
python3 scripts/local-day.py
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
../day/target/debug/day --project demo launch -p macos-appkit \
  --script demo/dayscript/demo.yaml --capture-size window
```

The ordinary macOS build has no release key/embedded updater until the packaging hook runs. For a local signed fixture, independent of GitHub or release credentials:

```sh
python3 scripts/prototype.py test
python3 scripts/prototype.py prepare --mode sandbox --skip-build
open 'build/prototype/sandbox/installed/Self Update Demo.app'
```

Use `--mode plain` to compare the nonsandboxed fixture. Fixture keys and bundles live under ignored `build/`. The fixture intentionally uses ad-hoc signatures and identifier-only XPC requirements; official tag builds bind XPC to the Apple anchor and configured signing team.

On Linux or Windows, `python3 scripts/desktop_fixture.py` exercises the native helper with synthetic signed packages, including bad-signature rejection and relaunch. The Windows test requires Rust and NSIS. CI retains these fast tests and also runs a real packaged **1.0.0 → 1.1.0** update on all three desktop runners, including sandboxed macOS. See [the testing guide](docs/TESTING.md) for coverage, filesystem feeds and local commands.

## Components

| Location | Purpose |
| --- | --- |
| `src/` | Reusable Day panel, the Updates settings provider and scheduler, GitHub client, portable installer launch |
| `crates/update-core/` | Shared Ed25519 verification, release contract, recoverable transactions, desktop helper logic |
| `tools/` | Key generation, signing/verifying, macOS helper and standalone Windows/Linux helper |
| `platform/macos/` | Small Foundation/XPC bridge; no general-purpose IPC framework dependency |
| `demo/` | Localized app and standard Day project |
| `scripts/` | Reproducible platform fixtures and secret-free release staging |

No Sparkle or `self_update` dependency is required by these backends. This is still an experimental updater: native release-signed validation, broader interruption/permissions testing, key rotation, channels, store URL handoff, and deltas remain outstanding. See the release guide for the exact supported package formats, signing boundaries, and recovery commands.
