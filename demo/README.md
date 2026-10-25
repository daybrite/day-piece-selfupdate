# Self Update Demo

A standard localized Day app for macOS AppKit, Windows WinUI, and Linux GTK. It checks the latest stable release of `daybrite/day-piece-self-update`, downloads a verified update, and lets the user quit to install and reopen it.

The app installs `Preferences` with Day's General group; the **Updates** group (automatic
checks, frequency, background downloads, last checked, Check now) and the **Check for Updates…**
menu item come from the `day-piece-self-update` settings provider. The home page mirrors the
updater's status and has a button that opens the group.

See [signing and release setup](../docs/RELEASING.md) before publishing. The macOS app is sandboxed. Initial Windows installations use the per-user NSIS package; Linux uses the AppImage. Flatpak, Snap, and MSIX installations do not self-replace.

For local UI development, from this directory:

```sh
../../day/target/debug/day launch -p macos-appkit --script dayscript/demo.yaml
```

For the local macOS fixture without release credentials:

```sh
cd ..
python3 scripts/prototype.py prepare --mode sandbox
open 'build/prototype/sandbox/installed/Self Update Demo.app'
```

Open Settings ▸ Updates, check, download, then quit within 90 seconds. The fixture moves from 0.1.0 to 0.2.0. Diagnostic `--selfupdate-probe` and `--selfupdate-install` entry points use the same native client as the UI.


## Generated native projects

Use a Day CLI with automatic native project generation; Apple builds require **Xcode 27
or newer**. Xcode project documents, Android Gradle files, and HarmonyOS hvigor files are
created under `build/` by `day prepare`, `day build`, and `day launch`. Open the selected
project with `day open -t <target>`; edits to generated files are disposable. Apple Swift
sources, plists and `.xcconfig` files, and any Android resource overlays, remain app-owned
under `platform/`. For additional native configuration, use
`day project add-target <target> --persist-native-project` to create a persistent override.
