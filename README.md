# Hierophant

[![CodeQL](https://github.com/unattended-backpack/hierophant/actions/workflows/codeql.yml/badge.svg)](https://github.com/unattended-backpack/hierophant/actions/workflows/codeql.yml) [![Create Release](https://github.com/unattended-backpack/hierophant/actions/workflows/release.yml/badge.svg)](https://github.com/unattended-backpack/hierophant/actions/workflows/release.yml)

> Prove all things; hold fast that which is good. 

Hierophant is an open-source SP1 prover network that is built to be a drop in replacement for a Succinct prover network endpoint.

[SP1](https://github.com/succinctlabs/sp1) is [Succinct's](https://www.succinct.xyz/) zero-knowledge virtual machine (zkVM). Hierophant was built to be directly compatible with [op-succinct](https://github.com/succinctlabs/op-succinct/), but any program that utilizes the [sp1-sdk](https://crates.io/crates/sp1-sdk) to request proofs can instead use a Hierophant instance.

Hierophant saves costs and maintains censorship-resistance over centralized prover network offerings, making it well-suited for truly-unstoppable applications.

## Hierophant and Contemplant

> "A [hierophant](https://en.wikipedia.org/wiki/Hierophant) is an interpreter of sacred mysteries and arcane principles."

The Hierophant is master of a self-hosted ZK prover network. The Hierophant receives proof requests and delegates them to any available Contemplants.

> "[Contemplant](https://en.wiktionary.org/wiki/contemplant): one who contemplates."

The Contemplant is slave to a self-hosted ZK prover network. A Contemplant actually performs the work of generating a proof request that is forwarded from a Hierophant.

# Quickstart

To get up and running quickly, we recommend visiting the [Scriptory](https://github.com/unattended-backpack/scriptory) to utilize our prepared setup for easily running a Hierophant, [Magister](https://github.com/unattended-backpack/magister), and a number of Contemplants.

If you would like to run a simple Hierophant and Contemplant pair, we provide a [setup here](./docker-compose.run.yml). Simply run `make init`, adjust your `hierophant.toml` and `contemplant.toml` files as desired, and run `make run` to use it. This will provide you with a working endpoint on the specified Docker network that you can use as the `NETWORK_RPC_URL` in programs that request proofs, such as the [Supplicant](https://github.com/unattended-backpack/supplicant/). Do note that you will also need to specify a `NETWORK_PRIVATE_KEY` as well.

You can test a full example of this by running `make integration`, and check the test [program](./src/fibonacci/) or [compose file](./docker-compose.test.yml).

# Standalone Hierophant

You can build a native version of Hierophant via `make build`. You can supply configuration to this Hierophant as either environment variables, or through a `hierophant.toml` created with `make init`. Please observe the available configuration in [`hierophant.example.toml`](./hierophant.example.toml).

Running Hierophant by itself doesn't do anything. Hierophant is the master who receives proof requests and assigns them to be executed by Contemplants. There is always just one Hierophant for many Contemplants, and you must run at least one Contemplant to successfully execute proofs.  

Once at least one Contemplant is connected, there is nothing else to do besides request a proof to the Hierophant. It will automatically route the proof to a Contemplant for work, and will return the proof when complete. Do note however that the execution loop of Hierophant is driven by receiving proof status requests from the [sp1-sdk](https://crates.io/crates/sp1-sdk), so make sure to poll for a proof status often. You likely don't have to worry about this as services that request SP1 proofs will by default poll frequently enough.

## Multiple Configuration Files

If you're running in an environment with multiple configuration files (for example, running an integration test while debugging), you can specify a specific config file with `-- --config <file>`.

## Hierophant Endpoints

Hierophant exposes several endpoints for basic status checking. They are all available on the HTTP port (default `9010`).

```bash
curl --request GET --url http://127.0.0.1:9010/contemplants
curl --request GET --url http://127.0.0.1:9010/dead-contemplants
curl --request GET --url http://127.0.0.1:9010/proof-history
```

- `GET /contemplants`: returns as JSON information on all Contemplants connected to this Hierophant. This includes IP, name, time alive, strikes, average proof completion time, the current in-progress proof, and progress on that proof.
- `GET /dead-contemplants`: returns as JSON information on all Contemplants that have been dropped by this Hierophant. Reasons for drop could be network disconnection, not making fast enough progress, or returning incorrect data.
- `GET /proof-history`: returns as JSON information on all proofs that this Hierophant has completed, including the Contemplant who was assigned to the proof, and the proof time.

# Standalone Contemplant

You can build a native version of Contemplant via `make build`. You can supply configuration to this Contemplant as either environment variables, or through a `contemplant.toml` created with `make init`. Please observe the available configuration in [`contemplant.example.toml`](./contemplant.example.toml).

A Contemplant must have a Hierophant to connect to. If the machine running Contemplant does not have Docker, Contemplant has to be run with the `enable-native-gnark` feature. Otherwise, proofs will fail to verify. Building using this setting is the default behavior of our [Makefile](./Makefile).

## SSH Access

To aid in debugging running Contemplant images, they make themselves accessible via SSH. Add your SSH keys to `container/authorized_keys` if you want SSH access inside Contemplant.

## Prover Types

Contemplant supports two prover types:

- **CPU proving** (`prover_type = "cpu"`, default): this uses the CPU for proving. No GPU is required, but proof generation will be **significantly** slower than GPU proving. This is suitable for development and testing.
- **CUDA proving** (`prover_type = "cuda"`): this uses the GPU for proof acceleration. This requires a CUDA-capable GPU with compute capability 8.6 or higher (NVIDIA RTX 30-series or newer).

### Progress Tracking Limitation

**Progress tracking is only available when using `prover_type = "cuda"` with a remote `moongate_endpoint`.**

The following configurations do **not** support progress tracking:
- CPU proving (`prover_type = "cpu"`)
- Dockerized CUDA proving (`prover_type = "cuda"` without `moongate_endpoint`)
- Mock proving (when proof requests have `mock = true`)

**Important:** Hierophant's `worker_required_progress_interval_mins` configuration defaults to `0` (disabled). If you want Hierophant to drop workers that don't report progress within a certain interval, you must:
1. Ensure all your Contemplants use `prover_type = "cuda"` with a `moongate_endpoint`.
2. Set `worker_required_progress_interval_mins` to a non-zero value in your Hierophant configuration.

## Multiple Configuration Files

If you're running in an environment with multiple configuration files (for example, running an integration test while debugging), you can specify a specific config file with `-- --config <file>`.

# Architecture

This is what the general network architecture will look like when following the quickstart and using the [Scriptory](https://github.com/unattended-backpack/scriptory).

![Scriptory Diagram](media/sigil-prover-network.png)

## State Scheme

Most of state is handled in a non-blocking actor pattern where one thread holds state and others interact with state by sending messages to that thread. Any module in this repo that contain the files `client.rs` and `command.rs` is following this pattern. These modules are `contemplant/proof_store`, `hierophant/artifact_store`, and `hierophant/worker_registry`.

The flow of control for these modules are `client method → command → handler → state update/read`. To add a new state-touching function, first add a new command to the appropriate `command.rs` file. Let the Rust compiler guide you through the rest of the implementation. In the future we'd like to move to a more robust actor library like Actix or Ractor.

### Contemplant

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

### Hierophant

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

## Shared `network-lib`

```bash
src/
├── lib.rs                 # Shared structs
├── messages.rs            # Shared message types 
└── protocol.rs            # Shared protocol constants
```

## Developing

When making a breaking change in inter-Hierophant-Contemplant communication, increment the `CONTEMPLANT_VERSION` variable in `network-lib/src/lib.rs`. On each Contemplant connection, the Hierophant asserts that the `CONTEMPLANT_VERSION` matches.

If file structure is changed, kindly update the architecture tree for readability.

When a new version of SP1 is released, extract the new `moongate-server` binary from Succict's CUDA prover docker image and update the checksum at `container/moongate-server.tar.gz.sha256`. Moongate is the name of Succinct's closed source CUDA proof accelerator. This checksum allows the actual binary to be downloaded from its supply-chain-hardened location at `VENDOR_BASE_URL`.

When supporting a new version, you must also update the vendored [circuits](./circuits/) using their own detailed [instructions](./circuits/README.md). Then, update the `CIRCUITS_VERSION` in `.env.maintainer`.

### Integration test

The integration test is a basic configuration that only tests minimal compatibility. It runs a Hierophant with one Contemplant and requests a single small fibonacci proof. Run it with `make integration`.

## Building

To build both the Hierophant and Contemplant native binaries, you need to install [`protoc`](https://protobuf.dev/installation/) and [Go](https://go.dev/doc/install). Then, you should simply need to run `make`; you can see more in the [`Makefile`](./Makefile). This will default to building with the maintainer-provided details from [`.env.maintainer`](./.env.maintainer), which we will periodically update as details change.

You can also build a Docker image using `make docker`, which uses a `BUILD_IMAGE` for building dependencies that are packaged to run in a `RUNTIME_IMAGE`. Configuration values in `.env.maintainer` may be overridden by specifying them as environment variables.
```bash
IMAGE_NAME=hierophant
BUILD_IMAGE=registry.digitalocean.com/sigil/petros:latest make build
RUNTIME_IMAGE=debian:bookworm-slim@sha256:... make build
```

### Building Container Images

You can build container images via either `make docker` or via `make ci` after building native binaries. Check the [Makefile](./Makefile) goals for more detailed information.

## Configuration

Our configuration follows a zero-trust model where all sensitive configuration is stored on the self-hosted runner, not in GitHub. This section documents the configuration required for automated releases via GitHub Actions.

Running this project may require some sensitive configuration to be provided in `.env` and other files; you can generate the configuration files from the provided examples with `make init`. Review configuration files carefully and populate all required fields before proceeding.

### Runner-Local Secrets

All automated build secrets must be stored on the self-hosted runner at `/opt/github-runner/secrets/`. These files are mounted read-only into the release workflow container; they are never stored in git.

#### Required Secrets

**GitHub Access Tokens** (for creating releases and pushing to GHCR):
- `ci_gh_pat` - A GitHub fine-grained personal access token with repository permissions.
- `ci_gh_classic_pat` - A GitHub classic personal access token for GHCR authentication.

**Registry Access Tokens** (for pushing container images):
- `do_token` - A DigitalOcean API token with container registry write access.
- `dh_token` - A Docker Hub access token.

**GPG Signing Keys** (for signing release artifacts):
- `gpg_private_key` - A base64-encoded GPG private key for signing digests.
- `gpg_passphrase` - The passphrase for the GPG private key.
- `gpg_public_key` - The base64-encoded GPG public key (included in release notes).

**Registry Configuration** (`registry.env` file):

This file contains non-sensitive registry identifiers and build configuration:

```bash
# The Docker image to perform release builds with.
# If not set, defaults to unattended/petros:latest from Docker Hub.
# Examples:
#   BUILD_IMAGE=registry.digitalocean.com/sigil/petros:latest
#   BUILD_IMAGE=ghcr.io/your-org/petros:latest
#   BUILD_IMAGE=unattended/petros:latest
BUILD_IMAGE=unattended/petros:latest

# The runtime base image for the final container.
# If not set, uses the value from from .env.maintainer.
# Example:
#   RUNTIME_IMAGE=debian:trixie-slim@sha256:66b37a5078a77098bfc80175fb5eb881a3196809242fd295b25502854e12cbec
RUNTIME_IMAGE=debian:trixie-slim@sha256:66b37a5078a77098bfc80175fb5eb881a3196809242fd295b25502854e12cbec

# The name of the DigitalOcean registry to publish the built image to.
DO_REGISTRY_NAME=

# The username of the Docker Hub account to publish the built image to.
DH_USERNAME=unattended
```

### Public Configuration

Public configuration that anyone building this project needs is stored in the repository at [`.env.maintainer`](./.env.maintainer):
- `HIEROPHANT_NAME` - The name of the Hierophant image.
- `CONTEMPLANT_NAME` - The name of the Contemplant image.
- `BUILD_IMAGE` - The builder image for compiling Rust code (default: `unattended/petros:latest`).
- `RUNTIME_IMAGE` - The runtime base image (default: pinned `debian:trixie-slim@sha256:...`).
- `VENDOR_BASE_URL` - The URL where large, specifically-vendored binaries are downloaded from.
- `CIRCUITS_VERSION` - The SP1 circuits version to use in the build.

This file is version-controlled and updated by maintainers as infrastructure details change.

## Verifying Release Artifacts

All releases include GPG-signed artifacts for verification. Each release contains:

- `image-digests.txt` - A human-readable list of container image digests.
- `image-digests.txt.asc` - A GPG signature for the digest list.
- `ghcr-manifest.json` / `ghcr-manifest.json.asc` - A GitHub Container Registry OCI manifest and signature.
- `dh-manifest.json` / `dh-manifest.json.asc` - A Docker Hub OCI manifest and signature.
- `do-manifest.json` / `do-manifest.json.asc` - A DigitalOcean Container Registry OCI manifest and signature.

### Quick Verification

Download the artifacts and verify signatures:

```bash
# Import the GPG public key (base64-encoded in release notes).
echo "<GPG_PUBLIC_KEY>" | base64 -d | gpg --import

# Verify digest list.
gpg --verify image-digests.txt.asc image-digests.txt

# Verify image manifests.
gpg --verify ghcr-manifest.json.asc ghcr-manifest.json
gpg --verify dh-manifest.json.asc dh-manifest.json
gpg --verify do-manifest.json.asc do-manifest.json
```

### Manifest Verification

The manifest files contain the complete OCI image structure (layers, config, metadata). You can use these to verify that a registry hasn't tampered with an image.
```bash
# Pull the manifest from the registry.
docker manifest inspect ghcr.io/unattended-backpack/...@sha256:... \
  --verbose > registry-manifest.json

# Compare to the signed manifest.
diff ghcr-manifest.json registry-manifest.json
```

This provides cryptographic proof that the image structure (all layers and configuration) matches what was signed at release time.

### Cosign Verification

Images are also signed with [cosign](https://github.com/sigstore/cosign) using GitHub Actions OIDC for keyless signing. This provides automated verification and build provenance.

To verify with cosign:
```bash
# Verify image signature (proves it was built by our workflow).
cosign verify ghcr.io/unattended-backpack/...@sha256:... \
  --certificate-identity-regexp='^https://github.com/unattended-backpack/.+' \
  --certificate-oidc-issuer=https://token.actions.githubusercontent.com
```

Cosign verification provides:
- Automated verification (no manual GPG key management).
- Build provenance (proves image was built by the GitHub Actions workflow).
- Registry-native signatures (stored alongside images).

**Note**: Cosign depends on external infrastructure (GitHub OIDC, Rekor). For maximum trust independence, rely on the GPG-signed manifests as your ultimate root of trust.

## Local Testing

This repository is configured to support testing the release workflow locally using the `act` tool. There is a corresponding goal in the Makefile, and instructions for further management of secrets [here](./docs/WORKFLOW_TESTING.md). This local testing file also shows how to configure the required secrets for building.

# Security

If you discover any bug; flaw; issue; dæmonic incursion; or other malicious, negligent, or incompetent action that impacts the security of any of these projects please responsibly disclose them to us; instructions are available [here](./SECURITY.md).

# License

The [license](./LICENSE) for all of our original work is `LicenseRef-VPL WITH AGPL-3.0-only`. This includes every asset in this repository: code, documentation, images, branding, and more. You are licensed to use all of it so long as you maintain _maximum possible virality_ and our copyleft licenses.

Permissive open source licenses are tools for the corporate subversion of libre software; visible source licenses are an even more malignant scourge. All original works in this project are to be licensed under the most aggressive, virulently-contagious copyleft terms possible. To that end everything is licensed under the [Viral Public License](./licenses/LicenseRef-VPL) coupled with the [GNU Affero General Public License v3.0](./licenses/AGPL-3.0-only) for use in the event that some unaligned party attempts to weasel their way out of copyleft protections. In short: if you use or modify anything in this project for any reason, your project must be licensed under these same terms.

For art assets specifically, in case you want to further split hairs or attempt to weasel out of this virality, we explicitly license those under the viral and copyleft [Free Art License 1.3](./licenses/FreeArtLicense-1.3).
