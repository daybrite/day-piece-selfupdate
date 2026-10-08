use day_selfupdate_core::{Release, Result, digest, hex, public_key, sign, verify, verify_archive};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};
#[cfg(target_os = "macos")]
mod macos;

fn main() {
    if let Err(e) = entry() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
fn entry() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("keygen") if args.len() == 2 => {
            let mut seed = [0; 32];
            getrandom::fill(&mut seed)?;
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            options.open(&args[1])?.write_all(hex(&seed).as_bytes())?;
            println!("{}", public_key(&hex(&seed))?);
        }
        Some("public-key") if args.len() == 2 => {
            println!("{}", public_key(&fs::read_to_string(&args[1])?)?)
        }
        Some("sign") if args.len() == 3 => println!(
            "{}",
            sign(&fs::read(&args[1])?, &fs::read_to_string(&args[2])?)?
        ),
        Some("verify-signature") if args.len() == 4 => {
            day_selfupdate_core::verify_signature(
                &fs::read(&args[1])?,
                &fs::read_to_string(&args[2])?,
                &args[3],
            )?;
            println!("verified");
        }
        Some("manifest") if args.len() == 6 => {
            let path = Path::new(&args[1]);
            let (size, sha256) = digest(path)?;
            let release = Release {
                schema: 1,
                application_id: args[2].clone(),
                version: args[3].clone(),
                build: args[4].parse()?,
                archive: path
                    .file_name()
                    .ok_or("missing filename")?
                    .to_str()
                    .ok_or("non UTF-8 filename")?
                    .into(),
                size,
                sha256,
                target: String::new(),
                tag: String::new(),
                helper: None,
            };
            let bytes = serde_json::to_vec_pretty(&release)?;
            fs::write(
                path.with_file_name("release.sig"),
                sign(&bytes, &fs::read_to_string(&args[5])?)?,
            )?;
            fs::write(path.with_file_name("release.json"), bytes)?;
        }
        Some("verify") if args.len() == 5 => {
            let inbox = Path::new(&args[1]);
            let release = verify(
                &fs::read(inbox.join("release.json"))?,
                &fs::read_to_string(inbox.join("release.sig"))?,
                &args[2],
                &args[3],
                args[4].parse()?,
            )?;
            verify_archive(&inbox.join(&release.archive), &release)?;
            println!("{}", serde_json::to_string(&release)?);
        }
        Some("recover-desktop") if args.len() == 3 => {
            let destination = std::path::PathBuf::from(&args[1]);
            let session = std::path::PathBuf::from(&args[2]);
            day_selfupdate_core::desktop::private_directory(&session)?;
            let lock = OpenOptions::new()
                .read(true)
                .write(true)
                .open(session.join("lock"))?;
            lock.try_lock()?;
            let t = day_selfupdate_core::Transaction {
                target: destination,
                stage: session.join("interrupted"),
                backup: session.join("previous"),
                journal: session.join("journal"),
            };
            println!("{}", t.recover()?);
        }
        Some("desktop-install") if args.len() == 2 => {
            let request = serde_json::from_slice(&day_selfupdate_core::desktop::read_limited(
                Path::new(&args[1]),
                65536,
            )?)?;
            day_selfupdate_core::desktop::install(&request)?;
        }
        _ => {
            #[cfg(target_os = "macos")]
            {
                return macos::entry(args);
            }
            #[cfg(not(target_os = "macos"))]
            {
                return Err("usage: keygen KEY | public-key KEY | sign FILE KEY | verify-signature FILE SIG PUBLIC_KEY | manifest ZIP ID VERSION BUILD KEY | verify INBOX PUBLIC_KEY ID BUILD | desktop-install REQUEST".into());
            }
        }
    }
    Ok(())
}
