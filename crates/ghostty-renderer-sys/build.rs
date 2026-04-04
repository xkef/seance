use std::env;
use std::path::PathBuf;

fn main() {
    let ghostty_dir = env::var("GHOSTTY_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
            manifest
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("ghostty")
        });

    let lib_dir = ghostty_dir.join("zig-out/lib");
    let include_dir = ghostty_dir.join("include");

    if !lib_dir.exists() {
        eprintln!(
            "ghostty not built yet. Run `zig build` in {} first.",
            ghostty_dir.display()
        );
        eprintln!("Then re-run `cargo build`.");
        std::process::exit(1);
    }

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static=ghostty-renderer");

    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
    println!("cargo:rustc-link-lib=framework=CoreGraphics");
    println!("cargo:rustc-link-lib=framework=CoreText");
    println!("cargo:rustc-link-lib=framework=CoreVideo");
    println!("cargo:rustc-link-lib=framework=QuartzCore");
    println!("cargo:rustc-link-lib=framework=IOSurface");
    println!("cargo:rustc-link-lib=framework=Metal");
    println!("cargo:rustc-link-lib=c++");

    println!("cargo:include={}", include_dir.display());
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=GHOSTTY_DIR");
}
