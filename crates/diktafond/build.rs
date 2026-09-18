use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn main() {
    println!("cargo:rerun-if-changed=swift/apple_intelligence.swift");
    println!("cargo:rerun-if-changed=swift/apple_intelligence_stub.swift");
    println!("cargo:rerun-if-changed=swift/apple_intelligence_bridge.h");
    // Which bridge gets compiled depends on the selected developer directory.
    println!("cargo:rerun-if-env-changed=DEVELOPER_DIR");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    let sdk = command_output("xcrun", &["--sdk", "macosx", "--show-sdk-path"])
        .expect("locating macOS SDK");
    let swiftc = command_output("xcrun", &["--find", "swiftc"]).expect("locating swiftc");
    let has_framework = Path::new(&sdk)
        .join("System/Library/Frameworks/FoundationModels.framework")
        .exists();
    let full_xcode = command_output("xcode-select", &["-p"])
        .is_some_and(|path| !path.ends_with("CommandLineTools"));
    let real_bridge = has_framework && full_xcode;
    let source = if real_bridge {
        "swift/apple_intelligence.swift"
    } else {
        "swift/apple_intelligence_stub.swift"
    };

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let object = out.join("apple_intelligence.o");
    let library = out.join("libapple_intelligence.a");
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86_64") => "x86_64",
        other => panic!("unsupported macOS architecture {other:?}"),
    };
    let target = format!("{arch}-apple-macosx14.0");
    let status = Command::new(&swiftc)
        .args([
            "-parse-as-library",
            "-target",
            &target,
            "-sdk",
            &sdk,
            "-O",
            "-import-objc-header",
            "swift/apple_intelligence_bridge.h",
            "-c",
            source,
            "-o",
        ])
        .arg(&object)
        .status()
        .expect("running swiftc");
    assert!(status.success(), "swiftc failed for {source}");

    let status = Command::new("libtool")
        .args(["-static", "-o"])
        .arg(&library)
        .arg(&object)
        .status()
        .expect("running libtool");
    assert!(status.success(), "libtool failed for {source}");

    let swift_lib = Path::new(&swiftc)
        .parent()
        .and_then(Path::parent)
        .expect("Swift toolchain root")
        .join("lib/swift/macosx");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-search=native={}", swift_lib.display());
    println!("cargo:rustc-link-search=native={sdk}/usr/lib/swift");
    println!("cargo:rustc-link-lib=static=apple_intelligence");
    println!("cargo:rustc-link-lib=framework=Foundation");
    if real_bridge {
        println!("cargo:rustc-link-arg=-weak_framework");
        println!("cargo:rustc-link-arg=FoundationModels");
    }
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
}
