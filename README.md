# Hierophant

Hierophant is a locally-hosted SP1 prover network that is built to be a drop in replacement as a Succinct prover network endpoint.

## "Hierophant" and "contemplant"

> "A hierophant is an interpreter of sacred mysteries and arcane principles."
[wikipedia](https://en.wikipedia.org/wiki/Hierophant)

> "Contemplant: One who contemplates."
[wikipedia](https://en.wiktionary.org/wiki/contemplant)

"Hierophant" and "Contemplant" are our versions of "coordinator" and "worker" or "master" and "slave".  The Hierophant receives proof requests and delegates them to be computed by the Contemplants.  "Contemplant" is used interchangeably with "worker" in this repo for brevity.

# Running Hierophant

Make a `hierophant.toml` and fill in config values:

```bash
cp hierophant/hierophant.example.toml hierophant/hierophant.toml
# Add your config values
RUST_LOG=info cargo run --release --bin hierophant
```

If you're running in an environment with multiple configurations (for example, running an integration test while debugging), you can specify the config file with `-- --config <config file name>`.

# Running Contemplant

Make a `contemplant.toml` and fill in config values:

```bash
cp hierophant/contemplant.example.toml hierophant/contemplant.toml
# Add your config values
RUST_LOG=info cargo run --release --bin contemplant
```

If you're running in an environment with multiple configurations (for example, running an integration test while debugging), you can specify the config file with `-- --config <config file name>`.

If you're running Contemplant in an environment that can't run docker, see the [Contemplant without Docker access](#contemplant-without-docker-access) section below.

# Architecture

Most of the state is handled in a non-blocking actor pattern.  1 thread holds state and others interact with state by sending messages to that thread.
Any module in this repo that contain files `client.rs` and `command.rs` is following this pattern.  These modules are `contemplant/proof_store`, `hierophant/artifact_store`, and `hierophant/worker_registry`.
The flow of control for these modules are `Client method → Command → Handler → State update/read`.  To add a new state-touching function, I recommend first adding a new command to the enum in the appropriate `command.rs` file and letting the rust compiler guide you through the rest of the implementation.  In the future I'd like to move to a more robust actor library like Actix or Ractor.

## `contemplant` overview

```bash
src/
├── api/
│   ├── connect_to_hierophant.rs       # WebSocket initialization with Hierophant
│   ├── http.rs                        # HTTP handlers
│   └── mod.rs
│
├── proof_executor/
│   ├── assessor.rs                    # Proof execution estimation assessment 
│   ├── executor.rs                    # Proof execution
│   └── mod.rs
│
├── proof_store/
│   ├── client.rs                      # Public proof store interface
│   ├── command.rs                     # ProofStore access commands
│   ├── mod.rs
│   └── store.rs                       # Local proof status storage
│
├── config.rs                          # Configuration 
├── main.rs                            # Entry point
├── message_handler.rs                 # Handles messages from Hierophant
└── worker_state.rs                    # Global Contemplant state
```

## `hierophant` overview

```bash
src/
├── api/
│   ├── grpc/
│   │   ├── create_artifact_service.rs     # ArtifactStore service handlers
│   │   ├── mod.rs
│   │   └── prover_network_service.rs      # ProverNetwork service handlers
│   │
│   ├── http.rs                            # HTTP handlers
│   ├── mod.rs
│   └── websocket.rs                       # WebSocket handlers
│
├── artifact_store/
│   ├── artifact_uri.rs                    # Artifact id struct
│   ├── client.rs                          # Public artifact store interface 
│   ├── command.rs                         # ArtifactStore access commands
│   ├── mod.rs
│   └── store.rs                           # Artifact reading & writing
│
├── proof/
│   ├── completed_proof_info.rs            # A proof struct
│   ├── mod.rs
│   ├── router.rs                          # Interface for assigning and retrieving proofs
│   └── status.rs                          # ProofStatus struct
│
├── worker_registry/
│   ├── client.rs                          # Public worker registry interface
│   ├── command.rs                         # WorkerRegistry access commands
│   ├── mod.rs
│   ├── registry.rs                        # Managing and communicating with Contemplants
│   └── worker_state.rs                    # WorkerState struct
│
├── config.rs                              # Hierophant configuration
├── hierophant_state.rs                    # Global Hierophant state
└── main.rs

proto/
├── artifact.proto                         # Protobuf definitions for ArtifactStore service
└── network.proto                          # Protobuf definitions for ProverNetwork service
```

## `network-lib` overview

This is a library for types shared between both `hierophant` and `contemplant`.

```bash
src/
├── lib.rs                 # Shared structs
├── messages.rs            # Shared message types 
└── protocol.rs            # Shared protocol constants
```

# Building and developing

## Developing

When making a breaking change in Hierophant/Contemplant compatability, increment
the `CONTEMPLANT_VERSION` var in `network-lib/src/lib.rs`.  On each Contemplant
connection, the Hierophant asserts that the `CONTEMPLANT_VERSION` that they have
is the same as the `CONTEMPLANT_VERSION` being passed in by the Contemplant.

If file structure is changed, kindly update the tree in the [Architecture](#architecture) section for readability.

### Integration test

The integration test is a basic configuration that only tests minimal compatibility.  It runs a Hierophant with 1 Contemplant and requests a single small proof.

```bash
RUST_LOG=info cargo run --release --bin integration-test
```

## Building

Install `protoc`: [https://protobuf.dev/installation/](https://protobuf.dev/installation/)

`cargo build --release`

### Contemplant without Docker access

If you're running a Contemplant in an environment where Docker containers can't run, like inside a Vast.ai instance, Succinct's CUDA prover `moongate` binary must be run separately and pointed to in `contemplant.toml` by setting a `moongate_endpoint`.  The `moongate` binary needs to be extracted from the latest Succinct CUDA prover image.  At the time of writing, that is `https://public.ecr.aws/succinct-labs/moongate:v5.0.0`.  You must also build the Contemplant binary with the feature `enable-native-gnark`.

```
cargo build --release --bin contemplant --features enable-native-gnark
```

## `old_testament.txt`

List of biblical names to randomly draw from if a Contemplant is started without a name.
