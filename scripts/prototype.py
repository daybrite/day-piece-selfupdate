#!/usr/bin/env python3
"""Build and exercise isolated, ad-hoc-signed macOS update fixtures. No release credentials."""
import argparse
import json
import os
from pathlib import Path
import plistlib
import shutil
import signal
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
DAY = ROOT.parent / 'day/target/debug/day'
TOOL = ROOT / 'target/debug/day-selfupdate-tool'
BUILD = ROOT / 'build/prototype'
APP_NAME = 'Self Update Demo.app'


def run(*args, **kwargs):
    return subprocess.run([str(a) for a in args], check=True, text=True, **kwargs)


def output(*args):
    return run(*args, capture_output=True).stdout.strip()


def write_plist(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(plistlib.dumps(data))


def info(app):
    return plistlib.loads((app / 'Contents/Info.plist').read_bytes())


def binary(app):
    return app / 'Contents/MacOS' / info(app)['CFBundleExecutable']


def build():
    run('cargo', 'build', '--workspace', '--offline', cwd=ROOT)
    run(DAY, '--project', ROOT / 'demo', 'build', '-p', 'macos-appkit', env={**os.environ, 'CARGO_NET_OFFLINE': 'true'})
    BUILD.mkdir(parents=True, exist_ok=True)
    run('xcrun', 'clang', '-fobjc-arc', '-fblocks', '-mmacosx-version-min=13.0',
        '-framework', 'Foundation', ROOT / 'platform/macos/service.m', '-o', BUILD / 'Launcher')


def sign(path, entitlements=None):
    args = ['/usr/bin/codesign', '--force', '--sign', '-']
    if entitlements:
        args += ['--entitlements', entitlements]
    run(*args, path, capture_output=True)


def stop_fixture(base):
    # Stop only demo executables at the exact generated fixture paths before rebuilding them.
    expected = str(base / 'installed' / APP_NAME / 'Contents/MacOS/Demo')
    for line in output('/bin/ps', '-axo', 'pid=,comm=').splitlines():
        fields = line.strip().split(None, 1)
        if len(fields) == 2 and fields[1] == expected:
            os.kill(int(fields[0]), signal.SIGTERM)
            time.sleep(.3)


def prepare(mode):
    candidates = list((ROOT / 'demo/build/day/macos-appkit').rglob('Demo.app'))
    if not candidates:
        raise RuntimeError('Build the standard demo first')
    source = candidates[0]
    base = BUILD / mode
    if base.exists():
        stop_fixture(base)
        shutil.rmtree(base)
    base.mkdir(parents=True)
    key = base / 'release.key'
    public = output(TOOL, 'keygen', key)
    app_id = 'dev.daybrite.selfupdatedemo.' + mode
    service_id = app_id + '.installer'
    entitlements = base / 'sandbox.plist'
    write_plist(entitlements, {'com.apple.security.app-sandbox': True})
    apps = []
    for folder, version, number in [('installed', '0.1.0', '1'), ('next', '0.2.0', '2')]:
        app = base / folder / APP_NAME
        app.parent.mkdir()
        run('/usr/bin/ditto', source, app)
        helpers = app / 'Contents/Helpers'
        helpers.mkdir(exist_ok=True)
        shutil.copy2(TOOL, helpers / TOOL.name)
        service = app / 'Contents/XPCServices' / (service_id + '.xpc')
        (service / 'Contents/MacOS').mkdir(parents=True)
        shutil.copy2(BUILD / 'Launcher', service / 'Contents/MacOS/Launcher')
        # Identifier-only requirements are deliberately development-only. Production must
        # require an Apple signing anchor and the publisher's Team ID on both peers.
        write_plist(service / 'Contents/Info.plist', {
            'CFBundleIdentifier': service_id, 'CFBundleExecutable': 'Launcher',
            'CFBundlePackageType': 'XPC!', 'CFBundleVersion': number,
            'XPCService': {'ServiceType': 'Application'},
            'DYSUClientRequirement': f'identifier "{app_id}"',
        })
        settings = info(app)
        settings.update(CFBundleIdentifier=app_id, CFBundleVersion=number,
                        CFBundleShortVersionString=version, LSMinimumSystemVersion='13.0',
                        DYSUPublicKey=public, DYSUDevelopment=True,
                        DYSUServiceIdentifier=service_id,
                        DYSUServiceRequirement=f'identifier "{service_id}"')
        write_plist(app / 'Contents/Info.plist', settings)
        sign(helpers / TOOL.name)
        sign(service)  # Explicitly unsandboxed XPC launcher; no elevation or root service.
        sign(app, entitlements if mode == 'sandbox' else None)
        run('/usr/bin/codesign', '--verify', '--deep', '--strict', app)
        apps.append(app)
    installed, new = apps
    config = json.loads(output(binary(installed), '--selfupdate-probe'))
    assert config['can_write_installation_parent'] == (mode == 'plain'), 'sandbox behavior differs from fixture mode'
    inbox = Path(config['inbox'])
    inbox.mkdir(parents=True, exist_ok=True)
    # Only remove known fixture files, never unrelated temporary data.
    for name in ['release.json', 'release.sig', 'update.zip', 'relaunched.json']:
        (inbox / name).unlink(missing_ok=True)
    run('/usr/bin/ditto', '--norsrc', '--noextattr', '--noqtn', '-c', '-k', '--keepParent', new, inbox / 'update.zip')
    run(TOOL, 'manifest', inbox / 'update.zip', app_id, '0.2.0', '2', key)
    (base / 'fixture.json').write_text(json.dumps(config, indent=2))
    print(f'{mode}: prepared {installed}', flush=True)
    return installed, inbox, config


def wait_for(predicate, seconds=45):
    until = time.monotonic() + seconds
    while time.monotonic() < until:
        result = predicate()
        if result:
            return result
        time.sleep(.1)
    raise TimeoutError('fixture condition did not complete')


def test(mode):
    app, inbox, config = prepare(mode)
    session = app.parent / ('.day-selfupdate-' + config['application_id'])
    # Re-sign only the client with an unexpected identity; the service must reject it.
    args = ['/usr/bin/codesign', '--force', '--sign', '-', '--identifier', config['application_id'] + '.wrong']
    if mode == 'sandbox':
        args += ['--entitlements', BUILD / mode / 'sandbox.plist']
    run(*args, app, capture_output=True)
    wrong = subprocess.run([str(binary(app)), '--selfupdate-install'], capture_output=True, text=True, timeout=50)
    assert wrong.returncode != 0, 'wrong caller identity accepted'
    assert not (session / 'ready').exists(), 'wrong caller reached installer'
    sign(app, BUILD / mode / 'sandbox.plist' if mode == 'sandbox' else None)
    print(f'{mode}: rejected wrong caller identity', flush=True)
    signature = (inbox / 'release.sig').read_text()
    (inbox / 'release.sig').write_text('00' * 64)
    rejected = subprocess.run([str(binary(app)), '--selfupdate-install'], capture_output=True, text=True, timeout=50)
    assert rejected.returncode != 0, 'invalid signature accepted'
    assert info(app)['CFBundleVersion'] == '1'
    print(f'{mode}: rejected tampered signature ({rejected.stderr.strip()})', flush=True)
    (inbox / 'release.sig').write_text(signature)
    result = run(binary(app), '--selfupdate-install', capture_output=True, timeout=50)
    assert result.stdout.strip() == 'ready', result
    def installed():
        path = session / 'outcome.json'
        if path.exists():
            state = json.loads(path.read_text())
            if state['state'] == 'failed':
                raise RuntimeError(state)
            return state['state'] == 'installed'
    wait_for(installed)
    assert info(app)['CFBundleVersion'] == '2'
    assert info(session / 'previous.app')['CFBundleVersion'] == '1'
    wait_for(lambda: (inbox / 'relaunched.json').exists())
    assert json.loads((inbox / 'relaunched.json').read_text())['build'] == '2'
    print(f'{mode}: PASS: helper survived caller exit; installed build 2; retained build 1; relaunched build 2', flush=True)
    stop_fixture(BUILD / mode)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['build', 'prepare', 'test'])
    parser.add_argument('--mode', choices=['plain', 'sandbox', 'both'], default='both')
    parser.add_argument('--skip-build', action='store_true')
    args = parser.parse_args()
    if not args.skip_build:
        build()
    if args.action != 'build':
        for mode in (['plain', 'sandbox'] if args.mode == 'both' else [args.mode]):
            (test if args.action == 'test' else prepare)(mode)


if __name__ == '__main__':
    main()
