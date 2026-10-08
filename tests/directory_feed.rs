#![cfg(feature = "github")]
//! Synthetic signed fixtures; no network or native installer needed.
use day_piece_selfupdate::{
    Configuration,
    github::{Source, check_from, download_update_from},
    verification::{self, Release},
};
use std::fs;

#[test]
fn directory_transport_keeps_authentication_and_download_checks() {
    let temp = tempfile::tempdir().unwrap();
    let feed = temp.path().join("feed");
    let tag = feed.join("v1.1.0");
    let inbox = temp.path().join("inbox");
    fs::create_dir_all(&tag).unwrap();
    fs::create_dir(&inbox).unwrap();
    let key = "01".repeat(32);
    let mut config = Configuration {
        application_id: "test.fixture".into(),
        build: "1".into(),
        version: "1.0.0".into(),
        key: verification::public_key(&key).unwrap(),
        inbox: inbox.clone(),
        bundle: temp.path().join("fixture.app"),
        development: true,
        repository: String::new(),
        target: "macos-appkit-aarch64".into(),
    };
    fs::write(
        feed.join("latest.json"),
        br#"{"tag_name":"v1.1.0","draft":false,"prerelease":false}"#,
    )
    .unwrap();
    let artifact = tag.join("fixture.zip");
    fs::write(&artifact, b"synthetic package bytes").unwrap();
    let (size, sha256) = verification::digest(&artifact).unwrap();
    let release = Release {
        schema: 2,
        application_id: config.application_id.clone(),
        version: "1.1.0".into(),
        build: 2,
        archive: "fixture.zip".into(),
        size,
        sha256,
        target: config.target.clone(),
        tag: "v1.1.0".into(),
        helper: None,
    };
    let metadata = serde_json::to_vec(&release).unwrap();
    let name = format!("update-{}.json", config.target);
    let signature_path = tag.join(format!("{name}.sig"));
    fs::write(tag.join(&name), &metadata).unwrap();
    let signature = verification::sign(&metadata, &key).unwrap();
    fs::write(&signature_path, &signature).unwrap();
    let source = Source::Directory(feed.clone());
    let update = check_from(&config, &source).unwrap().unwrap();
    assert!(
        !inbox.join("fixture.zip").exists(),
        "discovery must not download"
    );
    download_update_from(&config, &update, &source).unwrap();
    assert_eq!(
        fs::read(inbox.join("fixture.zip")).unwrap(),
        fs::read(&artifact).unwrap()
    );
    assert_eq!(fs::read(inbox.join("release.json")).unwrap(), metadata);
    fs::remove_file(inbox.join("fixture.zip")).unwrap();
    fs::write(&artifact, vec![b'x'; size as usize]).unwrap();
    assert!(download_update_from(&config, &update, &source).is_err());
    assert!(!inbox.join("fixture.zip").exists());
    fs::write(&artifact, vec![b'x'; size as usize + 1]).unwrap();
    assert!(download_update_from(&config, &update, &source).is_err());
    fs::write(&signature_path, "00".repeat(64)).unwrap();
    assert!(check_from(&config, &source).is_err());
    fs::write(&signature_path, signature).unwrap();
    config.build = "2".into();
    assert!(check_from(&config, &source).unwrap().is_none());
    config.build = "3".into();
    assert!(check_from(&config, &source).unwrap().is_none());
    config.application_id = "wrong.app".into();
    assert!(check_from(&config, &source).is_err());
    fs::write(
        feed.join("latest.json"),
        br#"{"tag_name":"../escape","draft":false,"prerelease":false}"#,
    )
    .unwrap();
    assert!(check_from(&config, &source).is_err());
}
