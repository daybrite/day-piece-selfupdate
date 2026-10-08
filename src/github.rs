//! GitHub release discovery. All downloads are bound to one immutable tag, never mixed /latest URLs.
use crate::{
    Configuration,
    verification::{self, Asset, Release, Result},
};
use serde::Deserialize;
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub struct Update {
    pub release: Release,
    pub metadata: Vec<u8>,
    pub signature: String,
}
#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
}

fn repository(config: &Configuration) -> Result<&str> {
    let parts: Vec<_> = config.repository.split('/').collect();
    if parts.len() != 2 || !parts.iter().all(|p| verification::safe_name(p)) {
        return Err("invalid GitHub repository".into());
    }
    Ok(&config.repository)
}
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(300)))
        .https_only(true)
        .user_agent("day-piece-selfupdate/0.1")
        .build()
        .into()
}
/// Explicit transport selection. Directory feeds contain `latest.json` and
/// `<tag>/update-<target>.json`, its `.sig`, and the signed release's artifacts.
/// Selecting a directory changes transport only; all signature, identity, size,
/// hash, target and increasing-build checks still apply. No environment override.
#[derive(Clone, Debug)]
pub enum Source {
    Github,
    Directory(PathBuf),
}
impl Source {
    fn open(&self, config: &Configuration, tag: Option<&str>, file: &str) -> Result<Box<dyn Read>> {
        if !verification::safe_name(file) || tag.is_some_and(|t| !verification::safe_name(t)) {
            return Err("unsafe release path".into());
        }
        match self {
            Self::Directory(root) => {
                // Canonical containment also rejects symlinks escaping the feed.
                let root = root.canonicalize()?;
                let path = match tag {
                    Some(tag) => root.join(tag).join(file),
                    None => root.join(file),
                }
                .canonicalize()?;
                if !path.starts_with(&root) {
                    return Err("release path escapes feed".into());
                }
                Ok(Box::new(File::open(path)?))
            }
            Self::Github => {
                let repo = repository(config)?;
                let url = match tag {
                    Some(tag) => {
                        format!("https://github.com/{repo}/releases/download/{tag}/{file}")
                    }
                    None => format!("https://api.github.com/repos/{repo}/releases/latest"),
                };
                Ok(Box::new(
                    agent().get(&url).call()?.into_body().into_reader(),
                ))
            }
        }
    }
    fn get(
        &self,
        config: &Configuration,
        tag: Option<&str>,
        file: &str,
        max: u64,
    ) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        self.open(config, tag, file)?
            .take(max + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > max {
            return Err("response exceeds limit".into());
        }
        Ok(bytes)
    }
}

pub fn check(config: &Configuration) -> Result<Option<Update>> {
    check_from(config, &Source::Github)
}

pub fn check_from(config: &Configuration, source: &Source) -> Result<Option<Update>> {
    let info: GithubRelease =
        serde_json::from_slice(&source.get(config, None, "latest.json", 1024 * 1024)?)?;
    if info.draft || info.prerelease {
        return Err("latest endpoint returned an unpublished or prerelease build".into());
    }
    let name = format!("update-{}.json", config.target);
    let metadata = source.get(config, Some(&info.tag_name), &name, 65536)?;
    let signature = String::from_utf8(source.get(
        config,
        Some(&info.tag_name),
        &format!("{name}.sig"),
        1024,
    )?)?;
    let release = verification::verify(
        &metadata,
        &signature,
        &config.key,
        &config.application_id,
        0,
    )?;
    if release.schema != 2 || release.target != config.target || release.tag != info.tag_name {
        return Err("release platform or tag mismatch".into());
    }
    if release.build <= config.build.parse()? {
        return Ok(None);
    }
    Ok(Some(Update {
        release,
        metadata,
        signature,
    }))
}

fn download(
    config: &Configuration,
    source: &Source,
    tag: &str,
    asset: &Asset,
    destination: &Path,
) -> Result<()> {
    let reader = source.open(config, Some(tag), &asset.name)?;
    let mut file = File::create(destination)?;
    let copied = std::io::copy(&mut reader.take(asset.size + 1), &mut file)?;
    file.sync_all()?;
    if copied != asset.size {
        return Err("download length mismatch".into());
    }
    verification::desktop::verify_file(destination, asset)
}

pub fn download_update(config: &Configuration, update: &Update) -> Result<()> {
    download_update_from(config, update, &Source::Github)
}

pub fn download_update_from(
    config: &Configuration,
    update: &Update,
    source: &Source,
) -> Result<()> {
    // Verify even if a caller constructed Update itself; never accept an unauthenticated URL.
    let release = verification::verify(
        &update.metadata,
        &update.signature,
        &config.key,
        &config.application_id,
        config.build.parse()?,
    )?;
    if release.target != config.target {
        return Err("release target mismatch".into());
    }
    let stage = tempfile::tempdir_in(&config.inbox)?;
    download(
        config,
        source,
        &release.tag,
        &Asset {
            name: release.archive.clone(),
            size: release.size,
            sha256: release.sha256.clone(),
        },
        &stage.path().join(&release.archive),
    )?;
    if let Some(helper) = &release.helper {
        download(
            config,
            source,
            &release.tag,
            helper,
            &stage.path().join(&helper.name),
        )?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                stage.path().join(&helper.name),
                std::fs::Permissions::from_mode(0o700),
            )?;
        }
    }
    for entry in std::fs::read_dir(stage.path())? {
        let entry = entry?;
        let destination = config.inbox.join(entry.file_name());
        if destination.exists() {
            std::fs::remove_file(&destination)?;
        }
        std::fs::rename(entry.path(), destination)?;
    }
    verification::atomic_write(&config.inbox.join("release.json"), &update.metadata)?;
    verification::atomic_write(
        &config.inbox.join("release.sig"),
        update.signature.as_bytes(),
    )?;
    Ok(())
}
