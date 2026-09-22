use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn setup_memory_x() {
    // 1. memory.x: copy into OUT_DIR and put it on the linker search path.
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::copy("memory.x", out.join("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");
}

fn emit_linker_args() {
    // 2. Linker args: link.x from cortex-m-rt, defmt.x from defmt.
    //    `link-rp.x` is deliberately absent: it only defines the `.boot2` section,
    //    and the app must not embed boot2 (the bootloader owns boot2).
    println!("cargo:rustc-link-arg-bins=--nmagic");
    println!("cargo:rustc-link-arg-bins=-Tlink.x");
    println!("cargo:rustc-link-arg-bins=-Tdefmt.x");
}

fn emit_version(manifest_dir: &Path) {
    // 3. Version identity: the repo-root VERSION file holds the single integer
    //    the firmware reports at runtime. Re-run when it changes, and fail
    //    loudly if it is not a bare integer.
    let version_path = manifest_dir.join("../VERSION");
    println!("cargo:rerun-if-changed={}", version_path.display());
    let raw = fs::read_to_string(&version_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", version_path.display()));
    let value = raw.trim();
    let version: u32 = value.parse().unwrap_or_else(|_| {
        panic!("VERSION must contain a single bare integer, found \"{value}\"")
    });
    println!("cargo:rustc-env=GARAGEDOOR_BUILD_VERSION={version}");
}

fn check_secrets(manifest_dir: &Path) {
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
}

fn fetch_firmware_blobs(manifest_dir: &Path) {
    // 5. CYW43 firmware blobs: retrieve if missing into app/firmware/
    let firmware_dir = manifest_dir.join("firmware");
    fs::create_dir_all(&firmware_dir).unwrap_or_else(|e| {
        panic!(
            "failed to create firmware directory {}: {e}",
            firmware_dir.display()
        )
    });

    let blobs = [
        (
            "43439A0.bin",
            "https://raw.githubusercontent.com/embassy-rs/embassy/3cd51e6d8eb6aff8b0d64d9e56a75a538bcfc65a/cyw43-firmware/43439A0.bin",
        ),
        (
            "43439A0_clm.bin",
            "https://raw.githubusercontent.com/embassy-rs/embassy/3cd51e6d8eb6aff8b0d64d9e56a75a538bcfc65a/cyw43-firmware/43439A0_clm.bin",
        ),
        (
            "nvram_rp2040.bin",
            "https://raw.githubusercontent.com/embassy-rs/embassy/3cd51e6d8eb6aff8b0d64d9e56a75a538bcfc65a/cyw43-firmware/nvram_rp2040.bin",
        ),
    ];

    for (name, url) in blobs {
        download_blob_if_missing(&firmware_dir, name, url);
    }
}

fn download_blob_if_missing(firmware_dir: &Path, name: &str, url: &str) {
    let dest = firmware_dir.join(name);
    println!("cargo:rerun-if-changed={}", dest.display());
    if dest.exists() {
        let len = fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
        if len > 0 {
            return;
        }
        let _ = fs::remove_file(&dest);
    }

    let tmp_dest = firmware_dir.join(format!("{name}.tmp"));
    let status = std::process::Command::new("curl")
        .args(["-sSfL", "-o", tmp_dest.to_str().unwrap(), url])
        .status()
        .unwrap_or_else(|e| panic!("failed to execute curl to download {url}: {e}"));
    if !status.success() {
        let _ = fs::remove_file(&tmp_dest);
        panic!("failed to download {url} via curl (exit status: {status})");
    }
    fs::rename(&tmp_dest, &dest).unwrap_or_else(|e| {
        panic!(
            "failed to rename {} to {}: {e}",
            tmp_dest.display(),
            dest.display()
        )
    });
}

fn main() {
    setup_memory_x();
    emit_linker_args();

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    emit_version(&manifest_dir);
    check_secrets(&manifest_dir);
    fetch_firmware_blobs(&manifest_dir);

    println!("cargo:rerun-if-changed=build.rs");
}
