# Signing and testing official demo releases

The repository is `daybrite/day-piece-selfupdate`. Its `release-signing` GitHub environment admits `v*` tags. No source has been committed or pushed by this task, and no release or signing credential has been published. The changes to `daybrite/actions` must be reviewed and published before this workflow can use their new inputs through `@v1`.

## Credentials: two independent signatures

The **update signature** is Ed25519 and authenticates the metadata, package hash, and installer-helper hash on every platform. It is independent of macOS Developer ID, notarization, and Windows Authenticode. The prototype uses raw Ed25519 signatures in hexadecimal, not an SSH, GPG, Minisign, or Sparkle signature envelope.

Create your personal update key locally, outside any checkout, and back it up securely. From the selfupdate project:

```sh
cargo build -p day-selfupdate-tools
mkdir -p ~/.config/day/signing
chmod 700 ~/.config/day/signing
target/debug/day-selfupdate-tool keygen ~/.config/day/signing/selfupdate-demo.key
```

The command prints only the **public** key; the file contains the private 32-byte seed as 64 hex characters (0600 on Unix). Do not share or commit the file. Store the private seed as an environment secret and the public key as a repository variable:

```sh
gh secret set DAY_UPDATE_PRIVATE_KEY \
  --repo daybrite/day-piece-selfupdate --env release-signing \
  < ~/.config/day/signing/selfupdate-demo.key

gh variable set DAY_UPDATE_PUBLIC_KEY --repo daybrite/day-piece-selfupdate \
  --body "$(target/debug/day-selfupdate-tool public-key ~/.config/day/signing/selfupdate-demo.key)"
```

An existing update key can be used if it has this exact seed format. Do not paste another format into this secret. Key rotation is not implemented: preserve the same key for both the initially installed release and its successors. Losing it prevents those installations from trusting future updates.

The Rust utility and runtime share `day-selfupdate-core`. Manual verification:

```sh
target/debug/day-selfupdate-tool sign artifact.zip ~/.config/day/signing/selfupdate-demo.key > artifact.zip.sig
target/debug/day-selfupdate-tool verify-signature artifact.zip artifact.zip.sig PUBLIC_KEY_HEX
```

The credential-isolated GitHub signer implements the same exact-byte Ed25519 format with Python's `cryptography`; it never executes a helper built by the application while holding signing credentials. The signed JSON covers SHA-256 and length of the final package and helper, including the final notarized macOS ZIP.

## App Fair Developer ID and notarization

Configure the same six secret names as Day-Showcase, in **this repository's** `release-signing` environment:

| Secret | Value |
| --- | --- |
| `DAY_MACOS_CERT_P12` | Base64 of the App Fair Developer ID Application certificate **and private key** exported as a `.p12` |
| `DAY_MACOS_CERT_PASSWORD` | Password used when exporting that `.p12` |
| `DAY_SIGN_MACOS_IDENTITY` | Full Developer ID Application identity name |
| `DAY_NOTARY_KEY_B64` | Base64 of the App Store Connect notary `.p8` key |
| `DAY_NOTARY_KEY_ID` | That API key's ID |
| `DAY_NOTARY_ISSUER` | Its issuer UUID |

GitHub exposes secret names, not values: the existing Day-Showcase environment cannot be copied by reading its API. Reuse the original credential files/password-manager entries, or enter them through Settings → Environments → release-signing. For example, on macOS:

```sh
base64 -i /path/to/AppFair-DeveloperID.p12 | gh secret set DAY_MACOS_CERT_P12 \
  --repo daybrite/day-piece-selfupdate --env release-signing
base64 -i /path/to/AuthKey_KEYID.p8 | gh secret set DAY_NOTARY_KEY_B64 \
  --repo daybrite/day-piece-selfupdate --env release-signing
# Interactive prompts avoid placing sensitive values in shell command history:
gh secret set DAY_MACOS_CERT_PASSWORD --repo daybrite/day-piece-selfupdate --env release-signing
gh secret set DAY_SIGN_MACOS_IDENTITY --repo daybrite/day-piece-selfupdate --env release-signing
gh secret set DAY_NOTARY_KEY_ID --repo daybrite/day-piece-selfupdate --env release-signing
gh secret set DAY_NOTARY_ISSUER --repo daybrite/day-piece-selfupdate --env release-signing
```

Also set the **public** `DAY_UPDATE_APPLE_TEAM` repository variable to the ten-character Team ID on that Developer ID certificate:

```sh
gh variable set DAY_UPDATE_APPLE_TEAM --repo daybrite/day-piece-selfupdate --body YOURTEAMID
```

This binds both XPC peers to the expected Apple signing anchor, Team ID, and bundle identifier. It must match the imported certificate. A tag build fails if public signing configuration is absent, and signing fails if the private credentials are absent or inconsistent.

The app is sandboxed with outgoing networking. Its embedded XPC launcher and Rust installer are separately signed and unsandboxed, without root privileges. The shared signing job signs nested code first, signs the app with its sandbox entitlements, notarizes/staples the DMG, then staples the app and creates the update ZIP. It uses Apple's documented [nested notarization and ZIP procedure](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow). Credentials are confined to jobs that do not build or execute repository code.

Windows packages currently use Day's default platform-signing behavior. Ed25519 update authentication does **not** make Windows trust an Authenticode publisher or eliminate SmartScreen prompts. Configure Day's Windows signing provider separately if you want a trusted Windows publisher signature.

## Release procedure

1. Publish the reviewed `daybrite/actions` changes so its `v1` workflow includes `post-pack-command`, `update-signing`, `update-public-key`, `update-application-id`, and `macos-update-zip`.
2. Commit and push the reviewed selfupdate project yourself, after credentials/public variables are configured. No commits or pushes were performed by the implementation task.
3. Set `demo/Cargo.toml`'s package version and increase `[app].build` in `demo/Day.toml`. Keep the root crate version aligned for clarity. The demo version must exactly match the tag without `v`. **Every update needs a strictly larger integer build number.** Do not derive ordering from GitHub publication time or workflow run number.
4. Create and push a matching tag, for example `v0.1.0`. The tag push starts the workflow; it builds/tests the crate and all three desktop demos, signs/notarizes macOS, signs updater metadata, and creates/publishes the GitHub release. Creating a release in GitHub's UI for an already-pushed tag is not the trigger used here.
5. Wait for the whole workflow to pass. Missing credentials fail the release; it does not silently advertise an unsigned update.
6. Install that official release. Then repeat with, for example, version `0.1.1` and build `2`, using the same update key and Apple team.

`project-path: demo` and `release-assets: true` (the default) already supported releases of `day-piece-*` and `day-part-*` demos. There is no special demo-only release mechanism; the new optional inputs add update artifacts and isolated signing to the existing workflow. A manual workflow dispatch on a tag also uses that tag's release path.

## What to install and test

| Platform | Initial installation | Update payload |
| --- | --- | --- |
| macOS | Open the notarized DMG; copy `Self Update Demo.app` into a writable Applications location; unmount the DMG | Complete Developer ID-signed, stapled `.app` ZIP |
| Windows | Run the `*-windows-winui-setup.exe` per-user NSIS installer | New NSIS installer, executed silently after the app exits, using its existing installation directory |
| Linux | Download `*-linux-gtk-<arch>.appimage`, make executable, and run from a writable location | New complete AppImage, staged on the destination filesystem |

Run Check → Prepare → Quit and install within 90 seconds. Confirm the version changes after relaunch. Existing user data is outside the replaced application files. The old bundle/file/directory remains in a private sibling `.day-selfupdate-<id>` directory. Asking for another update from the newly installed build confirms that prior replacement and permits removing its previous backup.

The client queries GitHub's **latest stable** release, then uses only that tag's metadata and artifact URLs. `update-<target>-<arch>.json` and `.json.sig` authenticate identity, target, tag, version, build, package name/size/hash, and the standalone Windows/Linux helper's name/size/hash. Prerelease selection and automatic checking/downloading remain future policy work. Avoid moving tags or replacing assets after publication.

Flatpak, Snap, and packaged Windows/MSIX installations are detected and directed to their package manager in the UI; they do not self-replace. Other permission failures preserve the current install where possible and produce diagnostics. This implementation does not elevate permissions or install a privileged service. Store URL handoff is not yet implemented.

## Validation and recovery

Before the shared release workflow runs, native CI builds two actual demo releases and verifies rejection of tampering, installation and relaunch on macOS, Windows and Linux. See [packaged integration testing](TESTING.md). These tests use disposable update keys and no production signing credentials.

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
python3 scripts/prototype.py test       # macOS: sandboxed and plain XPC fixtures
python3 scripts/desktop_fixture.py      # native Linux/Windows: real helper, signed synthetic package
```

Windows CI installs NSIS for its native integration test. Linux integration needs kernel pidfd support (Linux 5.3+). The helper waits on an OS process handle before replacing files. Do not mistake compilation on macOS for a native Windows installation test.

Portable helper failures are recorded in the per-user inbox's `outcome.json`. A machine/power failure between replacement steps can require explicit recovery. From the downloaded helper (or a locally built `day-selfupdate-tool`):

```text
day-selfupdate-tool recover-desktop DESTINATION SESSION_DIRECTORY
```

On macOS use `recover APP SESSION_DIRECTORY`. Recovery restores the backup when the journal is uncommitted, even if the destination disappeared. Successful NSIS installation is journaled; rollback restores the old application files on installer failure, but cannot transactionally undo arbitrary NSIS registry or shortcut changes. Multiple running instances, forced helper termination, full disks, and release-signed behavior need broader platform testing before treating this as a production updater.
