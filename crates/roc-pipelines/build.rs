//! Builds the DeckLink status helper (`native/dl_status.cpp`) when the `gst`
//! feature is on and the DeckLink SDK headers are present (default
//! `/usr/include/decklink`, override with `DECKLINK_SDK_INCLUDE`). Without the
//! headers the crate still builds; card-status format detection is disabled.

use std::path::PathBuf;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(roc_decklink_sdk)");
    println!("cargo:rerun-if-changed=native/dl_status.cpp");
    println!("cargo:rerun-if-env-changed=DECKLINK_SDK_INCLUDE");

    if std::env::var_os("CARGO_FEATURE_GST").is_none() {
        return;
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    let inc = std::env::var_os("DECKLINK_SDK_INCLUDE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/usr/include/decklink"));
    let dispatch = inc.join("DeckLinkAPIDispatch.cpp");
    if !inc.join("DeckLinkAPI.h").exists() || !dispatch.exists() {
        println!(
            "cargo:warning=DeckLink SDK headers not found in {} — card-status format detection disabled",
            inc.display()
        );
        return;
    }
    println!("cargo:rerun-if-changed={}", dispatch.display());

    cc::Build::new()
        .cpp(true)
        .std("c++14")
        .include(&inc)
        .file("native/dl_status.cpp")
        .file(&dispatch)
        // The SDK's dispatch source is not warning-clean; keep our build output quiet.
        .flag_if_supported("-Wno-unused-parameter")
        .flag_if_supported("-Wno-missing-field-initializers")
        .compile("roc_dl_status");
    println!("cargo:rustc-link-lib=dl");
    println!("cargo:rustc-link-lib=pthread");
    println!("cargo:rustc-cfg=roc_decklink_sdk");
}
