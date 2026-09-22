use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    // 1. memory.x: copy into OUT_DIR and put it on the linker search path.
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::copy("memory.x", out.join("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");

    // 2. Linker args: link.x from cortex-m-rt, defmt.x from defmt.
    //    `link-rp.x` is deliberately absent: it only defines the `.boot2` section,
    //    and the app must not embed boot2 (the bootloader owns boot2).
    println!("cargo:rustc-link-arg-bins=--nmagic");
    println!("cargo:rustc-link-arg-bins=-Tlink.x");
    println!("cargo:rustc-link-arg-bins=-Tdefmt.x");

    // 3. Version identity: the repo-root VERSION file holds the single integer
    //    the firmware reports at runtime. Re-run when it changes, and fail
    //    loudly if it is not a bare integer.
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let version_path = manifest_dir.join("../VERSION");
    println!("cargo:rerun-if-changed={}", version_path.display());
    let raw = fs::read_to_string(&version_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", version_path.display()));
    let value = raw.trim();
    let version: u32 = value.parse().unwrap_or_else(|_| {
        panic!("VERSION must contain a single bare integer, found \"{value}\"")
    });
    println!("cargo:rustc-env=GARAGEDOOR_BUILD_VERSION={version}");

    // 4. Secrets guard: `app/src/secrets.rs` is gitignored and materialised
    //    from the committed example. Fail actionably instead of building an
    //    image with no credentials.
    let secrets_path = manifest_dir.join("src/secrets.rs");
    if !secrets_path.exists() {
        panic!(
            "{} is missing; copy the template with: cp app/secrets.example.rs app/src/secrets.rs",
            secrets_path.display()
        );
    }
    println!("cargo:rerun-if-changed={}", secrets_path.display());

    println!("cargo:rerun-if-changed=build.rs");
}
