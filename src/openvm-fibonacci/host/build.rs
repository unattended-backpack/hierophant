// openvm-build cross-compiles the guest crate at ../guest to the
// riscv32im-risc0-zkvm-elf target and this script copies the resulting ELF
// into OUT_DIR, where main.rs embeds it via include_bytes!. Analogous to
// risc0-fibonacci's use of risc0_build::embed_methods().
//
// Toolchain: openvm-build shells out to `cargo +<toolchain>` where
// <toolchain> defaults to the OpenVM-pinned nightly (nightly-2026-01-18 for
// v2.0.1; override with OPENVM_RUST_TOOLCHAIN). Its preflight requires
// rustup to report the toolchain installed with rust-src; when missing it
// tries to install both through rustup, which needs network. Petros
// pre-materializes the vendored nightly (with its install manifests) under
// $RUSTUP_HOME, so builds inside petros, including ../Dockerfile, pass
// the preflight fully offline, matching the SP1/risc0 test clients.
//
// The guest is plain rv32im+io, so no openvm.toml and no openvm_init.rs are
// needed (init files are only generated for the modular/fp2/ecc extensions).

use openvm_build::{GuestOptions, build_guest_package, find_unique_executable, get_package};
use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let guest_dir = manifest_dir.join("../guest");

    println!("cargo:rerun-if-changed={}", guest_dir.join("src").display());
    println!(
        "cargo:rerun-if-changed={}",
        guest_dir.join("Cargo.toml").display()
    );

    // Reproducibility note: openvm-build itself honors OPENVM_BUILD_LOCKED
    // (set by ../Dockerfile) by appending --locked to the nested cargo
    // invocation, so the committed guest lockfile is respected without any
    // handling here.
    let guest_opts = GuestOptions::default();

    let pkg = get_package(&guest_dir);
    let target_dir = match build_guest_package(&pkg, &guest_opts, None, &None) {
        Ok(dir) => dir,
        Err(Some(code)) => panic!("OpenVM guest build failed with exit code {code}"),
        Err(None) => panic!("OpenVM guest build was skipped unexpectedly (OPENVM_SKIP_BUILD set?)"),
    };

    let elf_path = find_unique_executable(&guest_dir, &target_dir, &None)
        .expect("locate unique OpenVM guest ELF");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let dest = Path::new(&out_dir).join("fibonacci-guest.elf");
    std::fs::copy(&elf_path, &dest).unwrap_or_else(|e| {
        panic!(
            "copy guest ELF {} -> {}: {e}",
            elf_path.display(),
            dest.display()
        )
    });
}
