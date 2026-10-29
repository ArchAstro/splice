//! Builds splice-capture, the Swift helper that records the screen, camera
//! and microphone (Apple's capture frameworks are made for Swift), to be
//! carried inside the splice binary.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=capture/main.swift");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("splice-capture");
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("apple-darwin") {
        // Elsewhere there is nothing to record with.
        std::fs::write(&out, b"").expect("writing an empty helper");
        return;
    }
    let arch = if target.starts_with("aarch64") {
        "arm64"
    } else {
        "x86_64"
    };
    let status = Command::new("xcrun")
        .args(["--sdk", "macosx", "swiftc", "-O", "-swift-version", "5", "-target"])
        .arg(format!("{arch}-apple-macos13.0"))
        .arg("capture/main.swift")
        .arg("-o")
        .arg(&out)
        .status()
        .expect("running swiftc (install Xcode or its command line tools: xcode-select --install)");
    assert!(status.success(), "swiftc could not build capture/main.swift");
}
