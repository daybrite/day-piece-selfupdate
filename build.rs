fn main() {
    // The Updates group's private Fluent catalog (resource/locales/<lang>/app.ftl) becomes
    // `settings::res` through `day_fluent::locales!()`.
    #[cfg(feature = "settings")]
    day_build::generate_locales().expect("day-build: private locales");
    for name in [
        "DAY_UPDATE_PUBLIC_KEY",
        "DAY_UPDATE_APPLICATION_ID",
        "DAY_UPDATE_REPOSITORY",
        "DAY_UPDATE_BUILD",
        "DAY_UPDATE_VERSION",
        "DAY_UPDATE_TARGET",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    println!("cargo:rerun-if-changed=platform/macos/client.m");
    println!("cargo:rerun-if-changed=platform/macos/Protocol.h");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("platform/macos/client.m")
            .flag("-fobjc-arc")
            .flag("-fblocks")
            .flag("-mmacosx-version-min=13.0")
            .compile("day_selfupdate_client");
        println!("cargo:rustc-link-lib=framework=Foundation");
    }
}
