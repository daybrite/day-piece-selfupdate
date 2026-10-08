#!/usr/bin/env python3
"""Build and update two real Day release packages on the native desktop host.

All mutations are confined to build/e2e and the demo's private update inbox.
No GitHub publication, release credentials, or production update endpoint is used.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import re
import shutil
import subprocess
import sys
import time
import traceback
import uuid

ROOT = Path(__file__).resolve().parents[1]
TARGET = {'Darwin': 'macos-appkit', 'Windows': 'windows-winui', 'Linux': 'linux-gtk'}[platform.system()]
BASE = ROOT / 'build/e2e' / TARGET
WORK = BASE / 'workspace'
REPORTS = BASE / 'reports'
ENV = dict(os.environ)
# Never inherit release-tag validation, signing credentials or a foreign target directory.
for name in list(ENV):
    if name.startswith(('DAY_SIGN_', 'DAY_NOTARY_')) or name in ('GITHUB_ENV', 'CARGO_TARGET_DIR'):
        ENV.pop(name)
ENV.update(GITHUB_REF_TYPE='branch', APPIMAGE_EXTRACT_AND_RUN='1')


def run(*args, cwd=ROOT, capture=False, timeout=3600):
    print('+', ' '.join(map(str, args)), flush=True)
    return subprocess.run(list(map(str, args)), cwd=cwd, env=ENV, check=True,
                          text=True, capture_output=capture, timeout=timeout)


def write(path, value):
    path.write_text(json.dumps(value, indent=2))


def read(path):
    return json.loads(path.read_text())


def digest(path):
    with path.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def one(paths):
    paths = list(paths)
    if len(paths) != 1:
        raise AssertionError(f'expected one path, got {paths}')
    return paths[0]


def binary(app):
    info = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
    return app / 'Contents/MacOS' / info['CFBundleExecutable']


def build(day):
    if WORK.exists():
        # Preserve compiler caches between local runs, but refresh all source files.
        shutil.copytree(ROOT, WORK, dirs_exist_ok=True,
                        ignore=shutil.ignore_patterns('.git', 'build', 'target', '__pycache__', '*.key'))
    else:
        shutil.copytree(ROOT, WORK, ignore=shutil.ignore_patterns('.git', 'build', 'target', '__pycache__', '*.key'))
    # Only these isolated packages expose the automated driver.
    manifest = WORK / 'demo/Cargo.toml'
    text = manifest.read_text()
    for backend in ["appkit", "gtk", "winui"]:
        text = text.replace(f'{backend} = [', f'{backend} = ["e2e", ')
    manifest.write_text(text)
    day_toml = WORK / 'demo/Day.toml'
    day_toml.write_text(day_toml.read_text()
                       .replace('id = "dev.daybrite.selfupdatedemo"', 'id = "dev.daybrite.selfupdatedemo.e2e"')
                       .replace('title = "Self Update Demo"', 'title = "Self Update Demo E2E"'))
    run('cargo', 'build', '--release', '-p', 'day-selfupdate-tools', cwd=WORK)
    tool = WORK / 'target/release' / ('day-selfupdate-tool.exe' if os.name == 'nt' else 'day-selfupdate-tool')
    key = BASE / 'ephemeral.key'
    key.unlink(missing_ok=True)
    public = run(tool, 'keygen', key, capture=True).stdout.strip()
    ENV['DAY_UPDATE_PUBLIC_KEY'] = public
    for version, number in [('1.0.0', 1), ('1.1.0', 2)]:
        manifest.write_text(re.sub(r'^version = ".*"', f'version = "{version}"', text, count=1, flags=re.M))
        day_toml = WORK / 'demo/Day.toml'
        day_toml.write_text(re.sub(r'^build = \d+', f'build = {number}', day_toml.read_text(), count=1, flags=re.M))
        values = json.loads(run(sys.executable, WORK / 'scripts/release.py', 'configure', TARGET,
                                capture=True, cwd=WORK).stdout)
        ENV.update(values)
        dist = WORK / 'demo/build/day/dist'
        if dist.exists():
            shutil.rmtree(dist)
        formats = {'macos-appkit': 'dmg', 'windows-winui': 'nsis', 'linux-gtk': 'appimage'}[TARGET]
        run(day, '--project', WORK / 'demo', 'pack', '-p', TARGET, '--profile', 'release',
            '--formats', formats, '--no-sign', '--no-notarize', cwd=WORK / 'demo')
        run(sys.executable, WORK / 'scripts/release.py', 'stage', TARGET, cwd=WORK)
        saved = BASE / version
        if saved.exists():
            shutil.rmtree(saved)
        shutil.copytree(dist, saved)
    write(BASE / 'build.json', {'public_key': public, 'target': ENV['DAY_UPDATE_TARGET']})


def install_old():
    destination = BASE / 'installed'
    if destination.exists():
        shutil.rmtree(destination)
    destination.mkdir()
    old = BASE / '1.0.0'
    if TARGET == 'macos-appkit':
        mount = BASE / 'mounted'
        mount.mkdir(exist_ok=True)
        run('/usr/bin/hdiutil', 'attach', '-quiet', '-readonly', '-nobrowse', '-mountpoint', mount,
            one(old.glob('*.dmg')))
        try:
            source = one(mount.glob('*.app'))
            app = destination / source.name
            run('/usr/bin/ditto', source, app)
        finally:
            run('/usr/bin/hdiutil', 'detach', '-quiet', mount)
        run('/usr/bin/codesign', '--verify', '--deep', '--strict', app)
        entitlements = run('/usr/bin/codesign', '-d', '--entitlements', ':-', app, capture=True)
        assert plistlib.loads(entitlements.stdout.encode())['com.apple.security.app-sandbox'] is True
        return binary(app), app
    if TARGET == 'windows-winui':
        app = destination / 'Self Update Demo'
        # NSIS requires /D to be LAST and UNQUOTED, even if the path contains spaces.
        installer = one(old.glob('*-setup.exe'))
        subprocess.run(f'"{installer}" /S /D={app}', env=ENV, check=True, timeout=180)
        executables = [p for p in app.glob('*.exe') if not p.name.lower().startswith(('uninstall', 'crash'))]
        # The main Rust executable is named for the Cargo package by day pack.
        exe = app / 'selfupdate-demo.exe'
        if not exe.exists():
            exe = one(executables)
        return exe, app
    app = destination / 'Self Update Demo.appimage'
    shutil.copy2(one(old.glob('*.appimage')), app)
    app.chmod(0o755)
    return app, app


def probe(exe):
    result = run(exe, '--selfupdate-probe', capture=True, timeout=60)
    for line in result.stdout.splitlines():
        if line.startswith('{'):
            return json.loads(line)
    raise AssertionError(f'no configuration from probe: {result.stdout} {result.stderr}')


def wait(predicate, seconds=120):
    until = time.monotonic() + seconds
    while time.monotonic() < until:
        result = predicate()
        if result:
            return result
        time.sleep(.1)
    raise TimeoutError('timed out waiting for native app/update; see reports and installer outcome')


def exercise():
    config = read(BASE / 'build.json')
    tool = WORK / 'target/release' / ('day-selfupdate-tool.exe' if os.name == 'nt' else 'day-selfupdate-tool')
    key = BASE / 'ephemeral.key'
    exe, installed = install_old()
    original_hash = digest(exe)
    c = probe(exe)
    assert c['build'] == '1' and c['version'] == '1.0.0' and c['key'] == config['public_key'], c
    assert c['application_id'] == 'dev.daybrite.selfupdatedemo.e2e', c
    if TARGET == 'macos-appkit':
        assert c['can_write_installation_parent'] is False, 'app must actually be sandboxed'
    inbox = Path(c['inbox'])
    session = installed.parent / ('.day-selfupdate-' + c['application_id'])
    if session.exists():
        shutil.rmtree(session)
    # The feed lives inside the sandbox-accessible inbox, in its OWN directory.
    # Payloads are never pre-populated into the client's download cache.
    feed = inbox / ('e2e-feed-' + uuid.uuid4().hex)
    tag = feed / 'v1.1.0'
    shutil.copytree(BASE / '1.1.0', tag)
    name = f'update-{config["target"]}.json'
    release = read(tag / name.replace('.json', '.unsigned.json'))
    release['tag'] = 'v1.1.0'
    artifact = tag / release['archive']
    release.update(size=artifact.stat().st_size, sha256=digest(artifact))
    if release['helper']:
        helper = tag / release['helper']['name']
        release['helper'].update(size=helper.stat().st_size, sha256=digest(helper))
    write(tag / name, release)
    signature = run(tool, 'sign', tag / name, key, capture=True).stdout.strip()
    (tag / (name + '.sig')).write_text(signature)
    write(feed / 'latest.json', {'tag_name': 'v1.1.0', 'draft': False, 'prerelease': False})
    write(REPORTS / 'release.json', release)
    write(REPORTS / 'old-configuration.json', c)
    processes = []
    try:
        for scenario in ['bad-signature', 'bad-payload', 'success']:
            nonce = uuid.uuid4().hex
            for p in inbox.glob('e2e-*.json'):
                p.unlink()
            for filename in ['release.json', 'release.sig', release['archive'], 'outcome.json', 'ready']:
                (inbox / filename).unlink(missing_ok=True)
            write(inbox / 'e2e-request.json', {'nonce': nonce, 'feed': str(feed)})
            (tag / (name + '.sig')).write_text('00' * 64 if scenario == 'bad-signature' else signature)
            # Change one byte, preserving size: force the digest check, not just a length check.
            with artifact.open('rb') as f:
                first = f.read(1)
            if scenario == 'bad-payload':
                with artifact.open('r+b') as f:
                    f.write(bytes([first[0] ^ 1]))
            with (REPORTS / f'{scenario}.log').open('w') as log:
                child = subprocess.Popen([str(exe), '--selfupdate-e2e'], env=ENV, stdout=log, stderr=log)
                processes.append(child)
                if scenario != 'success':
                    wait(lambda: (inbox / 'e2e-error.json').exists())
                    child.wait(timeout=30)
                    assert digest(exe) == original_hash, 'rejected update changed installed executable'
                    assert not (inbox / 'e2e-downloaded.json').exists()
                    assert not (inbox / release['archive']).exists()
                    assert not (session / 'installed-build').exists()
                else:
                    def complete():
                        # Reap the old process promptly. macOS kill(pid, 0) also sees
                        # unreaped zombies; the harness must behave like LaunchServices.
                        child.poll()
                        error = inbox / 'e2e-error.json'
                        if error.exists():
                            raise AssertionError(read(error))
                        outcome = (session if TARGET == 'macos-appkit' else inbox) / 'outcome.json'
                        if outcome.exists() and read(outcome).get('state') == 'failed':
                            raise AssertionError(read(outcome))
                        path = inbox / 'e2e-complete.json'
                        return read(path) if path.exists() else None
                    proof = wait(complete, 180)
                    child.wait(timeout=30)
                    assert proof['nonce'] == nonce and proof['compiled_version'] == '1.1.0', proof
                    assert proof['configuration']['build'] == '2' and proof['reactive_root_started'], proof
                    expected_bundle = exe if os.name == 'nt' else installed
                    assert Path(proof['configuration']['bundle']) == expected_bundle, proof
                    started = read(inbox / 'e2e-started.json')
                    assert started['compiled_version'] == '1.0.0' and started['nonce'] == nonce, started
                    assert started['pid'] != proof['pid'], 'new process must relaunch'
                    for stage in ['available', 'downloaded', 'ready']:
                        assert read(inbox / f'e2e-{stage}.json')['nonce'] == nonce
                    assert digest(inbox / release['archive']) == release['sha256']
                    assert digest(exe) != original_hash, 'actual executable code must change'
                    backup = session / ('previous.app' if TARGET == 'macos-appkit' else 'previous')
                    backup_exe = binary(backup) if TARGET == 'macos-appkit' else (backup / exe.name if os.name == 'nt' else backup)
                    assert digest(backup_exe) == original_hash, 'original app must be retained for recovery'
                    assert (session / 'installed-build').read_text() == '2'
                    write(REPORTS / 'result.json', {'result': 'passed', 'target': TARGET, 'proof': proof,
                                                  'old_executable_sha256': original_hash,
                                                  'new_executable_sha256': digest(exe)})
            for p in inbox.glob('e2e-*.json'):
                shutil.copy2(p, REPORTS / f'{scenario}-{p.name}')
            if scenario == 'bad-payload':
                with artifact.open('r+b') as f:
                    f.write(first)
            print(f'{TARGET}: {scenario}: PASS', flush=True)
    finally:
        for child in processes:
            if child.poll() is None:
                child.terminate()
                child.wait(timeout=15)
        # Keep diagnostics even after failure; never upload the signing seed or payload tree.
        for directory, prefix in [(inbox, 'inbox'), (session, 'installer')]:
            if directory.exists():
                for p in directory.glob('*.json'):
                    shutil.copy2(p, REPORTS / f'{prefix}-{p.name}')
        shutil.rmtree(feed, ignore_errors=True)
        (inbox / 'e2e-request.json').unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--day', default=os.environ.get('DAY_BIN', 'day'))
    parser.add_argument('--reuse-builds', action='store_true', help='rerun previously built local fixtures')
    args = parser.parse_args()
    BASE.mkdir(parents=True, exist_ok=True)
    if REPORTS.exists():
        shutil.rmtree(REPORTS)
    REPORTS.mkdir()
    try:
        if not args.reuse_builds:
            build(args.day)
        exercise()
    except Exception:
        (REPORTS / 'failure.txt').write_text(traceback.format_exc())
        raise


if __name__ == '__main__':
    main()
