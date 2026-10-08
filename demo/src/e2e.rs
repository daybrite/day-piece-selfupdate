//! Test-only GUI automation. Compiled into isolated CI packages, never release builds.
use day_piece_selfupdate::{configuration, github, prepare, verification};

pub fn start() {
    let args: Vec<_> = std::env::args().collect();
    let updating = args.iter().any(|s| s == "--selfupdate-e2e");
    let relaunched = args.iter().any(|s| s == "--selfupdate-relaunched");
    if !updating && !relaunched {
        return;
    }
    std::thread::spawn(move || {
        // Give the native window/event loop time to mount the reactive root.
        std::thread::sleep(std::time::Duration::from_millis(500));
        let result = run(updating);
        if let Err(e) = result {
            eprintln!("e2e: {e}");
            if let Ok(c) = configuration() {
                let _ = verification::atomic_write(
                    &c.inbox.join("e2e-error.json"),
                    serde_json::json!({"error": e.to_string()})
                        .to_string()
                        .as_bytes(),
                );
            }
        }
        // Same UI-thread quit used by the demo's Quit to Install button.
        day::reactive::on_main(day::quit);
    });
}

fn run(updating: bool) -> verification::Result<()> {
    let c = configuration()?;
    let request: serde_json::Value =
        serde_json::from_slice(&std::fs::read(c.inbox.join("e2e-request.json"))?)?;
    let nonce = request["nonce"].as_str().ok_or("missing test nonce")?;
    let source =
        github::Source::Directory(request["feed"].as_str().ok_or("missing test feed")?.into());
    let report = |stage: &str| -> verification::Result<()> {
        verification::atomic_write(
            &c.inbox.join(format!("e2e-{stage}.json")),
            &serde_json::to_vec(&serde_json::json!({
                "nonce": nonce, "stage": stage, "pid": std::process::id(),
                "compiled_version": env!("CARGO_PKG_VERSION"),
                "configuration": c, "reactive_root_started": true,
            }))?,
        )
    };
    if updating {
        report("started")?;
        let update = github::check_from(&c, &source)?.ok_or("expected newer release")?;
        report("available")?;
        github::download_update_from(&c, &update, &source)?;
        report("downloaded")?;
        let outcome = prepare(&c)?;
        if outcome != "ready" {
            return Err(format!("installer did not become ready: {outcome}").into());
        }
        report("ready")?;
    } else {
        if github::check_from(&c, &source)?.is_some() {
            return Err("relaunched app still sees a newer release".into());
        }
        report("complete")?;
    }
    Ok(())
}
