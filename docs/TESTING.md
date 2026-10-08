# Self-update testing

The `ci` workflow has three layers. `host` runs Rust tests, formatting and Clippy on macOS, Windows and Linux, plus the fast synthetic Windows/Linux installer fixtures. `update-e2e` builds and exercises real native demo packages on all three hosts. The shared Day `demo` build/release workflow runs only after both layers pass; its dayscript checks the ordinary localized UI.

## Packaged app integration

`scripts/e2e.py` copies the project into ignored `build/e2e/<target>/workspace`, gives the fixture a separate application ID (`dev.daybrite.selfupdatedemo.e2e`) and title, and enables the demo's `e2e` feature only in that copy. It independently compiles release-profile versions **1.0.0 / build 1** and **1.1.0 / build 2** with `day pack` and the same `scripts/release.py stage` hook used by releases. Changing Cargo's package version changes the compiled Rust version constant; the test compares both running versions and executable hashes.

| Runner | First installation | Actual updater exercised |
| --- | --- | --- |
| macOS | Mount the generated DMG and copy its app | Sandboxed AppKit client → embedded XPC launcher → whole `.app` replacement → native relaunch |
| Windows | Run the generated per-user NSIS installer silently | Download signed helper and new NSIS installer → wait for old app to exit → reinstall at its existing directory → relaunch |
| Linux | Copy and execute the generated AppImage | Download signed helper and AppImage → pidfd wait → replace original AppImage → relaunch |

The harness generates an ephemeral Ed25519 key and signs schema-2 metadata with `day-selfupdate-tool`, the same Rust implementation used for runtime verification. No release credentials, published GitHub release or external update server is needed. macOS packages use ad-hoc platform signatures; the test asserts both the sandbox entitlement and actual denial of writes beside the installed app.

The local feed lives in a separate subdirectory of the app's private inbox, making it accessible to the sandboxed macOS client. Payloads are **not** preloaded into the download cache. Discovery, bounded streaming download, signature verification, identity/target/build checks, payload/helper hash verification, native installation and relaunch all run through the updater library and native backend.

Three scenarios launch the real native UI:

1. Invalid metadata signature: reject, retain the original executable, publish no downloaded package and never commit an installation.
2. Same-length corrupted package: reject its digest, with the same preservation assertions.
3. Valid update: discover 1.1, download, prepare the native installer, quit on the UI thread, replace and relaunch. The new app must enter its reactive UI root, report its **compiled** version as 1.1.0 from a different PID and the original installation path, and find no further update. The harness also checks the downloaded hash, changed executable hash, retained original backup and committed build number.

Per-run nonces prevent stale report files from satisfying the assertions. Failures and waits have deadlines. Reports and process logs are uploaded even on failure; private keys and package directories are excluded. These fixture packages are never published as releases. The feature-gated driver is absent from ordinary release/demo builds.

## Run locally

Use a native host with the same prerequisites as `day pack`: Rust, Python 3.11+, Day CLI, and the platform's build/packaging tools. Windows needs NSIS and the Windows App SDK; Linux needs GTK, linuxdeploy, Xvfb and D-Bus. CI provisions these with the existing shared setup actions.

```sh
python3 scripts/e2e.py --day /absolute/path/to/day
# Retry installation tests against the two packages already built:
python3 scripts/e2e.py --reuse-builds
```

For a headless Linux desktop session:

```sh
export XDG_RUNTIME_DIR="$(mktemp -d)"
chmod 700 "$XDG_RUNTIME_DIR"
GSK_RENDERER=cairo LIBGL_ALWAYS_SOFTWARE=1 \
  dbus-run-session -- xvfb-run -a python3 scripts/e2e.py
```

The harness sets `APPIMAGE_EXTRACT_AND_RUN=1`, so the genuine AppImage runtime works without a FUSE mount. Installation still replaces the original `.appimage` file, not an extracted executable. Results are under `build/e2e/<target>/reports/`; compiler caches and fixture artifacts remain under the same ignored build tree for diagnosis. The test temporarily replaces the dedicated E2E app's updater inbox contents; do not run two harness instances for the same user simultaneously.

## Filesystem feed API

The reusable client has an explicit transport override; it does not read ambient environment variables or silently fall back from HTTPS:

```rust,ignore
use day_piece_selfupdate::github::{Source, check_from, download_update_from};
let source = Source::Directory(feed_directory);
if let Some(update) = check_from(&configuration, &source)? {
    download_update_from(&configuration, &update, &source)?;
    // Call prepare off the UI thread, then let the app quit when ready.
}
```

A feed contains `latest.json` with `tag_name`, `draft` and `prerelease`, and a directory named by that tag containing `update-<target>-<arch>.json`, its `.json.sig`, and all artifacts named by the authenticated metadata. The same pinned public key applies to both transports. Paths are validated and canonicalized within the feed. Existing `github::check` and `github::download_update` continue using GitHub HTTPS. These APIs currently require the default `github` feature.

## Remaining coverage boundaries

This test proves a normal complete update and rejection of tampering using real packaged apps. It does not prove Developer ID/notarization/Gatekeeper or Authenticode/SmartScreen behavior, internet redirects/retries, every architecture or Linux distribution, managed-store updates, concurrent instances, disk exhaustion or all possible interruption points. The existing transaction tests separately kill child processes during both rename checkpoints and verify recovery. The older macOS `scripts/prototype.py test` adds wrong-XPC-caller rejection and a nonsandboxed comparison. Official signed-release testing remains necessary before treating the updater as production-ready.
