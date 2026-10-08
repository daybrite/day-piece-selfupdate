# Self Update Demo

A standard localized Day app for macOS AppKit, Windows WinUI, and Linux GTK. It checks the latest stable release of `daybrite/day-piece-selfupdate`, downloads a verified update, and lets the user quit to install and reopen it.

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

Check, prepare, then quit within 90 seconds. The fixture moves from 0.1.0 to 0.2.0. Diagnostic `--selfupdate-probe` and `--selfupdate-install` entry points use the same native client as the UI.
