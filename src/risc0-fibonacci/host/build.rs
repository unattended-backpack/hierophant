// risc0-build cross-compiles the guest crate(s) listed under
// [package.metadata.risc0].methods in Cargo.toml to the
// riscv32im-risc0-zkvm-elf target, then emits a `methods.rs` in OUT_DIR
// containing per-guest constants: `<CRATE>_ELF: &[u8]` and
// `<CRATE>_ID: [u32; 8]`. The host binary consumes these via
//   include!(concat!(env!("OUT_DIR"), "/methods.rs"));
//
// With GuestOptions::default() (what embed_methods uses), risc0-build runs
// the cross-compilation locally using the RISC Zero rust toolchain it
// discovers via the rzup library; NOT via rustup. That means the build
// environment must have the rzup layout under $HOME/.risc0/ pointing at a
// real toolchain. Petros provides this; see its Dockerfile.
fn main() {
    risc0_build::embed_methods();
}
