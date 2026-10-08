// Copyright © The Daybrite Project
// SPDX-License-Identifier: MPL-2.0

//! Prototype wire verification and recoverable bundle transaction. No UI or async runtime.
//! The detached signature covers the exact JSON bytes, not a reserialized representation.
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

pub mod desktop;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub schema: u32,
    pub application_id: String,
    pub version: String,
    pub build: u64,
    pub archive: String,
    pub size: u64,
    pub sha256: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub tag: String,
    #[serde(default)]
    pub helper: Option<Asset>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

pub fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}

pub fn validate_asset(asset: &Asset) -> Result<()> {
    if !safe_name(&asset.name) || asset.size == 0 || asset.size > 1024 * 1024 * 1024 {
        return Err("invalid asset name or size".into());
    }
    unhex::<32>(&asset.sha256)?;
    Ok(())
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex<const N: usize>(s: &str) -> Result<[u8; N]> {
    let s = s.trim();
    if s.len() != N * 2 || !s.is_ascii() {
        return Err("invalid hexadecimal length".into());
    }
    let mut bytes = [0; N];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16)?;
    }
    Ok(bytes)
}

pub fn public_key(secret: &str) -> Result<String> {
    Ok(hex(&SigningKey::from_bytes(&unhex(secret)?)
        .verifying_key()
        .to_bytes()))
}

pub fn sign(bytes: &[u8], secret: &str) -> Result<String> {
    Ok(hex(&SigningKey::from_bytes(&unhex(secret)?)
        .sign(bytes)
        .to_bytes()))
}

pub fn verify_signature(bytes: &[u8], signature: &str, key: &str) -> Result<()> {
    VerifyingKey::from_bytes(&unhex(key)?)?
        .verify_strict(bytes, &Signature::from_bytes(&unhex(signature)?))?;
    Ok(())
}

/// The caller supplies an installed trust anchor, never a key from the release.
pub fn verify(
    bytes: &[u8],
    signature: &str,
    key: &str,
    app_id: &str,
    build: u64,
) -> Result<Release> {
    if bytes.len() > 64 * 1024 {
        return Err("release metadata exceeds limit".into());
    }
    verify_signature(bytes, signature, key)?;
    let release: Release = serde_json::from_slice(bytes)?;
    if !matches!(release.schema, 1 | 2)
        || release.application_id != app_id
        || release.build <= build
    {
        return Err("schema, application identity, or version rejected".into());
    }
    let mut components = Path::new(&release.archive).components();
    if !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
        || !safe_name(&release.archive)
        || release.size == 0
        || release.size > 1024 * 1024 * 1024
    {
        return Err("invalid archive name or size".into());
    }
    unhex::<32>(&release.sha256)?;
    if release.schema == 1 {
        if !release.archive.ends_with(".zip") {
            return Err("legacy release must be a ZIP".into());
        }
    } else {
        if !safe_name(&release.tag) || !release.tag.starts_with('v') {
            return Err("invalid release tag".into());
        }
        let extension = if release.target.starts_with("macos-appkit-") {
            ".zip"
        } else if release.target.starts_with("windows-winui-") {
            ".exe"
        } else if release.target.starts_with("linux-gtk-") {
            ".appimage"
        } else {
            return Err("unsupported update target".into());
        };
        if !release.archive.ends_with(extension) {
            return Err("wrong package format".into());
        }
        if let Some(helper) = &release.helper {
            validate_asset(helper)?;
        } else if !release.target.starts_with("macos-") {
            return Err("missing installer helper".into());
        }
    }
    Ok(release)
}

pub fn digest(path: &Path) -> Result<(u64, String)> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut size = 0;
    let mut buf = [0; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
        size += n as u64;
    }
    Ok((size, hex(&hash.finalize())))
}

pub fn verify_archive(path: &Path, release: &Release) -> Result<()> {
    let (size, sha) = digest(path)?;
    if size != release.size || sha != release.sha256 {
        return Err("archive length or digest mismatch".into());
    }
    Ok(())
}

/// Durable small state files. All temporary paths are inside the controlled session directory.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("new");
    let mut file = File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(tmp, path)?;
    sync_parent(path)?;
    Ok(())
}

pub fn remove_path(path: &Path) -> Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn sync_parent(path: &Path) -> Result<()> {
    // Opening directories as std::fs::File is not supported on Windows.
    #[cfg(unix)]
    File::open(path.parent().ok_or("missing parent")?)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[derive(Debug)]
pub struct Transaction {
    pub target: PathBuf,
    pub stage: PathBuf,
    pub backup: PathBuf,
    pub journal: PathBuf,
}

impl Transaction {
    /// Caller must hold the installation lock and own the transaction directory.
    pub fn recover(&self) -> Result<bool> {
        let committed = fs::read(&self.journal).is_ok_and(|s| s == b"committed");
        if committed {
            return Ok(false);
        }
        if !self.backup.exists() {
            return Ok(false);
        }
        if self.target.exists() {
            if self.stage.exists() {
                remove_path(&self.stage)?;
            }
            fs::rename(&self.target, &self.stage)?;
        }
        fs::rename(&self.backup, &self.target)?;
        atomic_write(&self.journal, b"recovered")?;
        Ok(true)
    }

    /// Hook permits real process-kill fault injection in the fixture harness.
    /// A returned error triggers rollback; abrupt process death is handled by `recover`.
    pub fn replace(&self, mut checkpoint: impl FnMut(&str) -> Result<()>) -> Result<()> {
        if self.backup.exists() {
            return Err("previous backup needs confirmation".into());
        }
        atomic_write(&self.journal, b"prepared")?;
        let result = (|| {
            fs::rename(&self.target, &self.backup)?;
            sync_parent(&self.target)?;
            checkpoint("old-moved")?;
            fs::rename(&self.stage, &self.target)?;
            sync_parent(&self.target)?;
            checkpoint("new-moved")?;
            atomic_write(&self.journal, b"committed")?;
            Ok(())
        })();
        if result.is_err() {
            self.recover()?;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Release, String, String) {
        let secret = hex(&[42; 32]); // Synthetic test key, never shipped as a trust anchor.
        let release = Release {
            schema: 1,
            application_id: "test.app".into(),
            version: "2.0".into(),
            build: 2,
            archive: "update.zip".into(),
            size: 1,
            sha256: hex(&[0; 32]),
            target: String::new(),
            tag: String::new(),
            helper: None,
        };
        let public = public_key(&secret).unwrap();
        (release, secret, public)
    }

    #[test]
    fn accepts_python_release_signer_fixture_and_rejects_rollback() {
        // Synthetic protocol fixtures produced by actions/sign-updates, never application assets.
        let bytes = include_bytes!("../tests/data/update-linux-gtk-x86_64.json");
        let signature = include_str!("../tests/data/update-linux-gtk-x86_64.json.sig");
        let key = include_str!("../tests/data/public-key.txt");
        let release = verify(bytes, signature, key, "test.interop", 8).unwrap();
        assert_eq!(release.target, "linux-gtk-x86_64");
        assert_eq!(release.helper.unwrap().name, "helper");
        assert!(verify(bytes, signature, key, "test.interop", 9).is_err());
        assert!(verify(bytes, signature, key, "other.app", 8).is_err());
    }

    #[test]
    fn authenticates_identity_sequence_and_exact_bytes() {
        let (release, secret, public) = fixture();
        let bytes = serde_json::to_vec(&release).unwrap();
        let sig = sign(&bytes, &secret).unwrap();
        assert!(verify(&bytes, &sig, &public, "test.app", 1).is_ok());
        assert!(verify(&bytes, &sig, &public, "other.app", 1).is_err());
        assert!(verify(&bytes, &sig, &public, "test.app", 2).is_err());
        let mut changed = bytes.clone();
        changed.push(b' ');
        assert!(verify(&changed, &sig, &public, "test.app", 1).is_err());
        assert!(
            verify(
                &bytes,
                &sig,
                &public_key(&hex(&[43; 32])).unwrap(),
                "test.app",
                1
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_signed_path_traversal_and_large_payloads() {
        let (mut r, secret, public) = fixture();
        for name in ["../update.zip", "/tmp/update.zip", "nested/update.zip"] {
            r.archive = name.into();
            let bytes = serde_json::to_vec(&r).unwrap();
            assert!(
                verify(
                    &bytes,
                    &sign(&bytes, &secret).unwrap(),
                    &public,
                    "test.app",
                    1
                )
                .is_err()
            );
        }
        r.archive = "update.zip".into();
        r.size = 2 * 1024 * 1024 * 1024;
        let bytes = serde_json::to_vec(&r).unwrap();
        assert!(
            verify(
                &bytes,
                &sign(&bytes, &secret).unwrap(),
                &public,
                "test.app",
                1
            )
            .is_err()
        );
    }

    #[test]
    fn verifies_complete_payload() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("payload");
        fs::write(&p, b"fixture").unwrap();
        let (mut r, _, _) = fixture();
        (r.size, r.sha256) = digest(&p).unwrap();
        verify_archive(&p, &r).unwrap();
        fs::write(&p, b"changed").unwrap();
        assert!(verify_archive(&p, &r).is_err());
    }

    #[test]
    fn rolls_back_both_incomplete_swap_stages() {
        for failed in ["old-moved", "new-moved"] {
            let dir = tempfile::tempdir().unwrap();
            let t = Transaction {
                target: dir.path().join("app"),
                stage: dir.path().join("stage"),
                backup: dir.path().join("backup"),
                journal: dir.path().join("journal"),
            };
            fs::create_dir(&t.target).unwrap();
            fs::write(t.target.join("version"), b"old").unwrap();
            fs::create_dir(&t.stage).unwrap();
            fs::write(t.stage.join("version"), b"new").unwrap();
            assert!(
                t.replace(|phase| if phase == failed {
                    Err("fixture interruption".into())
                } else {
                    Ok(())
                })
                .is_err()
            );
            assert_eq!(fs::read(t.target.join("version")).unwrap(), b"old");
            t.replace(|_| Ok(())).unwrap();
            assert_eq!(fs::read(t.target.join("version")).unwrap(), b"new");
            assert!(!t.recover().unwrap());
            assert_eq!(fs::read(t.backup.join("version")).unwrap(), b"old");
        }
    }

    #[test]
    fn recovers_after_process_death() {
        for phase in ["old-moved", "new-moved"] {
            let dir = tempfile::tempdir().unwrap();
            let t = Transaction {
                target: dir.path().join("app"),
                stage: dir.path().join("stage"),
                backup: dir.path().join("backup"),
                journal: dir.path().join("journal"),
            };
            fs::create_dir(&t.target).unwrap();
            fs::write(t.target.join("version"), b"old").unwrap();
            fs::create_dir(&t.stage).unwrap();
            fs::write(t.stage.join("version"), b"new").unwrap();
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--ignored", "--exact", "tests::crash_worker"])
                .env("DYSU_TEST_ROOT", dir.path())
                .env("DYSU_TEST_PHASE", phase)
                .status()
                .unwrap();
            assert_eq!(status.code(), Some(91));
            assert!(t.recover().unwrap());
            assert_eq!(fs::read(t.target.join("version")).unwrap(), b"old");
            assert!(!t.recover().unwrap());
        }
    }

    #[test]
    #[ignore = "subprocess fixture used by recovers_after_process_death"]
    fn crash_worker() {
        let root = PathBuf::from(std::env::var_os("DYSU_TEST_ROOT").unwrap());
        let phase = std::env::var("DYSU_TEST_PHASE").unwrap();
        let t = Transaction {
            target: root.join("app"),
            stage: root.join("stage"),
            backup: root.join("backup"),
            journal: root.join("journal"),
        };
        t.replace(|p| {
            if p == phase {
                std::process::exit(91);
            }
            Ok(())
        })
        .unwrap();
        panic!("checkpoint was not reached");
    }
}
