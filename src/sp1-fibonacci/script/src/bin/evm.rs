//! End-to-end SP1 integration test driver against a Hierophant prover network.
//!
//! Requests a single fibonacci proof in the `--system`-selected mode, verifies
//! it client-side, and exits zero on success. Used by `make test-sp1`.
//!
//! The `--system` flag selects which of SP1's four proving modes to exercise:
//!   core        raw STARK, fastest to prove, not EVM-verifiable
//!   compressed  core STARK compressed into a single recursive proof
//!   plonk       compressed STARK wrapped into a Plonk SNARK (EVM-verifiable)
//!   groth16     compressed STARK wrapped into a Groth16 SNARK (EVM-verifiable)
//!
//! For Plonk and Groth16, the test client also writes a JSON fixture to
//! `src/sp1-fibonacci/contracts/src/fixtures/{plonk,groth16}-fixture.json`
//! containing `{vkey, publicValues, proof}` in the exact shape SP1's Solidity
//! verifier expects. These are intentionally not committed (the path is in
//! `.gitignore`); their purpose is to let an operator wire up an out-of-band
//! Foundry / Hardhat test against a stock SP1 verifier contract to confirm
//! the onchain path works for their deployment.
//!
//! Examples:
//! ```shell
//! RUST_LOG=info cargo run --release --bin evm -- --system groth16
//! RUST_LOG=info cargo run --release --bin evm -- --system core
//! ```
use alloy_sol_types::SolType;
use clap::{Parser, ValueEnum};
use fibonacci_lib::PublicValuesStruct;
use serde::{Deserialize, Serialize};
use sp1_sdk::{
    include_elf, utils, HashableKey, Prover, ProverClient, SP1ProofWithPublicValues, SP1Stdin,
    SP1VerifyingKey,
};
use std::path::PathBuf;

/// The ELF (executable and linkable format) file for the Succinct RISC-V zkVM.
pub const FIBONACCI_ELF: &[u8] = include_elf!("fibonacci-program");

/// The arguments for the EVM command.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct EVMArgs {
    #[arg(long, default_value = "20")]
    n: u32,
    #[arg(long, value_enum, default_value = "groth16")]
    system: ProofSystem,
}

/// Enum representing the available proof systems
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
enum ProofSystem {
    Core,
    Compressed,
    Plonk,
    Groth16,
}

/// A fixture that can be used to test the verification of SP1 zkVM proofs inside Solidity.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SP1FibonacciProofFixture {
    a: u32,
    b: u32,
    n: u32,
    vkey: String,
    public_values: String,
    proof: String,
}

fn main() {
    // Setup the logger.
    utils::setup_logger();

    // Parse the command line arguments.
    let args = EVMArgs::parse();
    println!("n: {}", args.n);
    println!("Proof System: {:?}", args.system);

    // Setup the prover client.
    let client = ProverClient::builder().network().build();

    // Setup the program.
    let (pk, vk) = client.setup(FIBONACCI_ELF);

    // Setup the inputs.
    let mut stdin = SP1Stdin::new();
    stdin.write(&args.n);

    // Generate the proof based on the selected proof system.
    let proof = match args.system {
        ProofSystem::Core => client.prove(&pk, &stdin).core().run(),
        ProofSystem::Compressed => client.prove(&pk, &stdin).compressed().run(),
        ProofSystem::Plonk => client.prove(&pk, &stdin).plonk().run(),
        ProofSystem::Groth16 => client.prove(&pk, &stdin).groth16().run(),
    }
    .expect("failed to generate proof");

    // Client-side verification for every mode. For Plonk/Groth16 this pulls
    // the gnark circuit artifacts from ~/.sp1/circuits/<mode>/<ver>/; our
    // Dockerfile vendors those into the image at build time (same tarballs
    // hierophant + contemplant use) so no network hit happens mid-test.
    client.verify(&proof, &vk).expect("proof verification failed");
    println!(
        "OK sp1 fibonacci(n={}) verified client-side [system={:?}]",
        args.n, args.system
    );

    // For the EVM-verifiable wraps, dump a fixture for out-of-band onchain
    // verification. `proof.bytes()` panics on core/compressed because those
    // modes have no EVM byte representation, so skip them.
    match args.system {
        ProofSystem::Plonk | ProofSystem::Groth16 => create_proof_fixture(&proof, &vk, args.system),
        ProofSystem::Core | ProofSystem::Compressed => {}
    }
}

/// Create a fixture for the given proof.
fn create_proof_fixture(
    proof: &SP1ProofWithPublicValues,
    vk: &SP1VerifyingKey,
    system: ProofSystem,
) {
    // Deserialize the public values.
    let bytes = proof.public_values.as_slice();
    let PublicValuesStruct { n, a, b } = PublicValuesStruct::abi_decode(bytes).unwrap();

    // Create the testing fixture so we can test things end-to-end.
    let fixture = SP1FibonacciProofFixture {
        a,
        b,
        n,
        vkey: vk.bytes32().to_string(),
        public_values: format!("0x{}", hex::encode(bytes)),
        proof: format!("0x{}", hex::encode(proof.bytes())),
    };

    // The verification key is used to verify that the proof corresponds to the execution of the
    // program on the given input.
    //
    // Note that the verification key stays the same regardless of the input.
    println!("Verification Key: {}", fixture.vkey);

    // The public values are the values which are publicly committed to by the zkVM.
    //
    // If you need to expose the inputs or outputs of your program, you should commit them in
    // the public values.
    println!("Public Values: {}", fixture.public_values);

    // The proof proves to the verifier that the program was executed with some inputs that led to
    // the give public values.
    println!("Proof Bytes: {}", fixture.proof);

    // Save the fixture to a file. The output directory is in `.gitignore`,
    // so the JSON written here never lands in a commit; it exists for
    // operators who want to wire it into an out-of-band Solidity verifier
    // test against SP1's onchain gateway.
    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../contracts/src/fixtures");
    std::fs::create_dir_all(&fixture_path).expect("failed to create fixture path");
    std::fs::write(
        fixture_path.join(format!("{:?}-fixture.json", system).to_lowercase()),
        serde_json::to_string_pretty(&fixture).unwrap(),
    )
    .expect("failed to write fixture");
}
