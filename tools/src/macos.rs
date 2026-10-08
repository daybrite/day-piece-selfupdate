// Copyright © The Daybrite Project
// SPDX-License-Identifier: MPL-2.0
use day_selfupdate_core::{
    Release, Result, Transaction, atomic_write, digest, hex, public_key, sign, verify,
    verify_archive,
};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn run(command: &mut Command) -> Result<()> {
    let output = command.output()?;
    if !output.status.success() {
        return Err(format!("tool failed: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    Ok(())
}

fn plist(bundle: &Path, key: &str) -> Result<String> {
    let out = Command::new("/usr/libexec/PlistBuddy")
        .arg("-c")
        .arg(format!("Print :{key}"))
        .arg(bundle.join("Contents/Info.plist"))
        .output()?;
    if !out.status.success() {
        return Err(format!("missing bundle key {key}").into());
    }
    Ok(String::from_utf8(out.stdout)?.trim().into())
}

fn bounded(path: &Path, max: u64) -> Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err("input is not a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err("input exceeds size limit".into());
    }
    Ok(bytes)
}

fn session_for(host: &Path) -> Result<PathBuf> {
    let id = plist(host, "CFBundleIdentifier")?;
    if !id
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || b".-".contains(&c))
    {
        return Err("invalid application ID".into());
    }
    Ok(host
        .parent()
        .ok_or("no installation parent")?
        .join(format!(".day-selfupdate-{id}")))
}

fn secure_directory(path: &Path) -> Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e.into()),
    }
    let meta = fs::symlink_metadata(path)?;
    // SAFETY: geteuid has no preconditions.
    if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
        return Err("unsafe session directory".into());
    }
    Ok(())
}

fn lock(session: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(session.join("lock"))?;
    // SAFETY: valid owned fd; the file remains alive for the lock's duration.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("another installation owns the session".into());
    }
    Ok(file)
}

fn transaction(host: &Path, session: &Path) -> Transaction {
    Transaction {
        target: host.into(),
        stage: session.join("stage").join(host.file_name().unwrap()),
        backup: session.join("previous.app"),
        journal: session.join("journal"),
    }
}

fn validate_bundle(path: &Path, host: &Path, release: &Release) -> Result<()> {
    run(Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict", "--deep"])
        .arg(path))?;
    if plist(path, "CFBundleIdentifier")? != release.application_id
        || plist(path, "CFBundleVersion")?.parse::<u64>()? != release.build
        || plist(path, "CFBundleShortVersionString")? != release.version
        || plist(path, "DYSUPublicKey")? != plist(host, "DYSUPublicKey")?
    {
        return Err("staged bundle does not match signed release or installed trust anchor".into());
    }
    let team = |bundle: &Path| -> Result<String> {
        let out = Command::new("/usr/bin/codesign")
            .args(["-dv", "--verbose=4"])
            .arg(bundle)
            .output()?;
        if !out.status.success() {
            return Err("cannot inspect bundle signature".into());
        }
        String::from_utf8(out.stderr)?
            .lines()
            .find_map(|line| line.strip_prefix("TeamIdentifier=").map(str::to_owned))
            .ok_or_else(|| "missing signing team".into())
    };
    let expected = team(host)?;
    if release.schema == 2 && plist(host, "DYSUTarget")? != release.target {
        return Err("release target mismatch".into());
    }
    if !matches!(plist(host, "DYSUDevelopment").as_deref(), Ok("true")) {
        if expected.len() != 10 || !expected.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err("invalid installed signing team".into());
        }
        let requirement = format!(
            "anchor apple generic and certificate leaf[subject.OU] = \"{expected}\" and identifier \"{}\"",
            release.application_id
        );
        run(Command::new("/usr/bin/codesign")
            .args(["--verify", "--strict", "-R", &requirement])
            .arg(path))?;
        run(Command::new("/usr/sbin/spctl")
            .args(["--assess", "--type", "execute"])
            .arg(path))?;
    }

    if team(path)? != expected {
        return Err("signing team mismatch".into());
    }
    if expected == "not set" && !matches!(plist(host, "DYSUDevelopment").as_deref(), Ok("true")) {
        return Err("ad-hoc signatures require explicit development fixture mode".into());
    }
    Ok(())
}

fn extract(payload: &Path, stage: &Path, app_name: &str) -> Result<()> {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let mut archive = zip::ZipArchive::new(File::open(payload)?)?;
    if archive.len() > 100_000 {
        return Err("too many archive entries".into());
    }
    if stage.exists() {
        fs::remove_dir_all(stage)?;
    }
    fs::create_dir(stage)?;
    let mut remaining = 2 * 1024 * 1024 * 1024_u64;
    let mut links = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let path = Path::new(entry.name());
        if !(path.starts_with(app_name)
            || path.starts_with(Path::new("__MACOSX").join(app_name))
            || path == Path::new("__MACOSX"))
            || path
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
            || !seen.insert(path.to_path_buf())
        {
            return Err("unexpected or duplicate archive path".into());
        }
        let destination = stage.join(path);
        remaining = remaining
            .checked_sub(entry.size())
            .ok_or("expanded archive exceeds limit")?;
        let mode = entry.unix_mode().unwrap_or(0o644);
        let kind = mode & 0o170000;
        if entry.is_dir() {
            fs::create_dir_all(&destination)?;
        } else if kind == 0o120000 {
            if !path.starts_with(app_name) {
                return Err("metadata symlink rejected".into());
            }
            if entry.size() > 4096 {
                return Err("symlink target exceeds limit".into());
            }
            let mut target = String::new();
            entry.read_to_string(&mut target)?;
            if Path::new(&target).is_absolute() {
                return Err("absolute symlink target".into());
            }
            links.push((destination, target));
        } else if kind == 0 || kind == 0o100000 {
            fs::create_dir_all(destination.parent().ok_or("missing archive parent")?)?;
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&destination)?;
            let limit = entry.size();
            let copied = std::io::copy(&mut entry.take(limit + 1), &mut output)?;
            if copied != limit {
                return Err("entry size mismatch".into());
            }
            output.set_permissions(fs::Permissions::from_mode(mode & 0o777))?;
            output.sync_all()?;
        } else {
            return Err("unsupported archive entry type".into());
        }
    }
    // Install links last: extraction never writes through an archive-supplied symlink.
    for (path, _) in &links {
        fs::create_dir_all(path.parent().ok_or("missing link parent")?)?;
    }
    let root = stage.join(app_name).canonicalize()?;
    for (path, target) in &links {
        symlink(target, path)?;
    }
    for (path, _) in links {
        if !path.canonicalize()?.starts_with(&root) {
            return Err("bundle symlink escapes root".into());
        }
    }
    // Restore AppleDouble metadata with the OS extractor only after validating every path,
    // size and symlink. This preserves stapled notarization metadata in official ZIPs.
    run(Command::new("/usr/bin/ditto")
        .args(["-x", "-k", "--rsrc", "--extattr"])
        .arg(payload)
        .arg(stage))?;
    Ok(())
}

fn install(host: &Path, inbox: &Path, pid: i32, session: &Path) -> Result<()> {
    if pid <= 1 {
        return Err("invalid caller process".into());
    }
    let t = transaction(host, session);
    t.recover()?;
    if t.backup.exists() {
        let installed = fs::read_to_string(session.join("installed-build"))?;
        if installed != plist(host, "CFBundleVersion")? || fs::read(&t.journal)? != b"committed" {
            return Err("previous update awaits explicit confirmation".into());
        }
        fs::remove_dir_all(&t.backup)?;
    }
    let key = plist(host, "DYSUPublicKey")?;
    let id = plist(host, "CFBundleIdentifier")?;
    let build = plist(host, "CFBundleVersion")?.parse()?;
    let metadata = bounded(&inbox.join("release.json"), 65536)?;
    let signature = String::from_utf8(bounded(&inbox.join("release.sig"), 1024)?)?;
    let release = verify(&metadata, &signature, &key, &id, build)?;
    let payload = session.join("payload.zip");
    // Copy from an opened regular file, then verify the private copy. Never verify one path
    // and later install bytes reopened from that untrusted path.
    let input = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(inbox.join(&release.archive))?;
    if !input.metadata()?.is_file() {
        return Err("archive is not a regular file".into());
    }
    let mut output = File::create(&payload)?;
    let n = std::io::copy(&mut input.take(release.size + 1), &mut output)?;
    output.sync_all()?;
    if n != release.size {
        return Err("unexpected archive size".into());
    }
    verify_archive(&payload, &release)?;
    extract(
        &payload,
        &session.join("stage"),
        host.file_name()
            .unwrap()
            .to_str()
            .ok_or("non UTF-8 bundle")?,
    )?;
    validate_bundle(&t.stage, host, &release)?;
    atomic_write(
        &session.join("ready"),
        std::process::id().to_string().as_bytes(),
    )?;
    let started = Instant::now();
    loop {
        // SAFETY: signal zero only probes the caller process; never signals it to exit.
        let live = unsafe { libc::kill(pid, 0) } == 0;
        if !live && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            break;
        }
        if started.elapsed() > Duration::from_secs(90) {
            return Err("caller did not exit; installation cancelled".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    t.replace(|_| Ok(()))?;
    atomic_write(
        &session.join("installed-build"),
        release.build.to_string().as_bytes(),
    )?;
    atomic_write(&session.join("outcome.json"), br#"{"state":"installed"}"#)?;
    run(Command::new("/usr/bin/open")
        .arg(host)
        .arg("--args")
        .arg("--selfupdate-relaunched"))?;
    Ok(())
}

pub fn entry(args: Vec<String>) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("keygen") if args.len() == 2 => {
            let mut secret = [0;32]; File::open("/dev/urandom")?.read_exact(&mut secret)?;
            let secret = hex(&secret);
            let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&args[1])?;
            file.write_all(secret.as_bytes())?;
            println!("{}", public_key(&secret)?);
        }
        Some("sign") if args.len() == 3 => {
            let bytes = fs::read(&args[1])?;
            println!("{}", sign(&bytes, &fs::read_to_string(&args[2])?)?);
        }
        Some("verify-signature") if args.len() == 4 => {
            day_selfupdate_core::verify_signature(
                &fs::read(&args[1])?,
                &String::from_utf8(bounded(Path::new(&args[2]), 1024)?)?,
                &args[3],
            )?;
            println!("verified");
        }
        Some("manifest") if args.len() == 6 => {
            let path = Path::new(&args[1]); let (size, sha256) = digest(path)?;
            let r = Release { schema: 1, application_id: args[2].clone(), version: args[3].clone(),
                build: args[4].parse()?, target: String::new(), tag: String::new(), helper: None, archive: path.file_name().unwrap().to_str().unwrap().into(), size, sha256 };
            let bytes = serde_json::to_vec_pretty(&r)?;
            fs::write(path.with_file_name("release.json"), &bytes)?;
            fs::write(path.with_file_name("release.sig"), sign(&bytes, &fs::read_to_string(&args[5])?)?)?;
        }
        Some("verify") if args.len() == 5 => {
            let inbox = Path::new(&args[1]);
            let r = verify(&bounded(&inbox.join("release.json"),65536)?,
                &fs::read_to_string(inbox.join("release.sig"))?, &args[2], &args[3], args[4].parse()?)?;
            verify_archive(&inbox.join(&r.archive), &r)?;
            println!("{}", serde_json::to_string(&r)?);
        }
        Some("install") if args.len() == 3 => {
            let exe = std::env::current_exe()?.canonicalize()?;
            let host = exe.ancestors().nth(3).ok_or("installer must be embedded in Contents/Helpers")?;
            if exe.parent().and_then(Path::file_name).and_then(|s| s.to_str()) != Some("Helpers") {
                return Err("installer must be embedded in Contents/Helpers".into());
            }
            let session = session_for(host)?; secure_directory(&session)?;
            let _lock = lock(&session)?;
            for f in ["ready", "outcome.json"] { let _ = fs::remove_file(session.join(f)); }
            let result = install(host, Path::new(&args[1]), args[2].parse()?, &session);
            if let Err(e) = &result {
                atomic_write(&session.join("outcome.json"), &serde_json::to_vec(&serde_json::json!({"state":"failed", "diagnostic":e.to_string()}))?)?;
            }
            result?;
        }
        Some("recover" | "confirm") if args.len() == 3 => {
            // Explicit maintenance command; not exposed over IPC and never privileged.
            // An explicit session also permits recovery when the target is temporarily absent.
            let host = PathBuf::from(&args[1]); let session = PathBuf::from(&args[2]);
            secure_directory(&session)?; let _lock = lock(&session)?;
            let t = transaction(&host, &session);
            if args[0] == "recover" { println!("{}", t.recover()?); }
            else {
                if fs::read(&t.journal)? != b"committed" { return Err("no committed update".into()); }
                if t.backup.exists() { fs::remove_dir_all(t.backup)?; }
                atomic_write(&t.journal, b"confirmed")?;
            }
        }
        _ => return Err("usage: keygen KEY | sign FILE KEY | verify-signature FILE SIGNATURE PUBLIC_KEY | manifest ZIP APP_ID VERSION BUILD KEY | verify INBOX PUBLIC_KEY APP_ID BUILD | install INBOX CALLER_PID | recover APP SESSION | confirm APP SESSION".into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;

    #[test]
    fn extracts_links_only_after_files_and_rejects_escape() {
        let temp = tempfile::tempdir().unwrap();
        let payload = temp.path().join("fixture.zip");
        for escape in [false, true] {
            let mut zip = zip::ZipWriter::new(File::create(&payload).unwrap());
            zip.start_file("Fixture.app/Contents/version", SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"new").unwrap();
            zip.add_symlink(
                "Fixture.app/link",
                if escape {
                    "../../outside"
                } else {
                    "Contents/version"
                },
                SimpleFileOptions::default(),
            )
            .unwrap();
            zip.finish().unwrap();
            fs::write(temp.path().join("outside"), b"untouched").unwrap();
            let result = extract(&payload, &temp.path().join("stage"), "Fixture.app");
            assert_eq!(result.is_err(), escape);
            assert_eq!(fs::read(temp.path().join("outside")).unwrap(), b"untouched");
        }
    }

    #[test]
    fn rejects_link_parent_before_writing_through_it() {
        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let payload = temp.path().join("fixture.zip");
        let mut zip = zip::ZipWriter::new(File::create(&payload).unwrap());
        zip.add_directory("Fixture.app/", SimpleFileOptions::default())
            .unwrap();
        zip.add_symlink(
            "Fixture.app/link",
            "../../outside",
            SimpleFileOptions::default(),
        )
        .unwrap();
        zip.add_symlink(
            "Fixture.app/link/child",
            "target",
            SimpleFileOptions::default(),
        )
        .unwrap();
        zip.finish().unwrap();
        assert!(extract(&payload, &temp.path().join("stage"), "Fixture.app").is_err());
        assert_eq!(fs::read_dir(outside).unwrap().count(), 0);
    }
}
