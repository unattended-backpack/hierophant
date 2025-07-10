# Hierophant

> "A hierophant is an interpreter of sacred mysteries and arcane principles."
[wikipedia](https://en.wikipedia.org/wiki/Hierophant)

Hierophant is a locally-hosted SP1 prover network that is built to be a drop in replacement as a Succinct prover network endpoint.

# Running Hierophant

Make a `hierophant.toml` and fill in config values:

```bash
cp hierophant/hierophant.example.toml hierophant/hierophant.toml
# Add your config values
```

# Running Contemplant

Make a `contemplant.toml` and fill in config values:

```bash
cp hierophant/contemplant.example.toml hierophant/contemplant.toml
# Add your config values
```

# Architecture

## `contemplant` overview

```bash
  contemplant/
  ├── Cargo.toml
  ├── src/
  │   ├── main.rs                    # Entry point 
  │   ├── config.rs                  # Configuration 
  │   │
  │   ├── connection/                # Connection management
  │   │   ├── mod.rs
  │   │   ├── websocket.rs           # WebSocket client 
  │   │   └── reconnect.rs           # Reconnection logic 
  │   │
  │   ├── proof/                     # Proof execution
  │   │   ├── mod.rs
  │   │   ├── executor.rs            # Proof generation 
  │   │   ├── assessor.rs            # Execution assessment 
  │   │   └── store.rs               # Local proof storage 
  │   │
  │   ├── message/                   # Message handling
  │   │   ├── mod.rs
  │   │   └── handler.rs             # Message routing 
  │   │
  │   └── monitoring/                # Resource monitoring
  │       ├── mod.rs
  │       └── resources.rs           # CPU/GPU assessor
```

## `hierophant` overview

```bash
 hierophant/
  ├── Cargo.toml
  ├── src/
  │   ├── main.rs                    # Application entry point 
  │   ├── config.rs                  # Configuration types 
  │   ├── state.rs                   # Global state management 
  │   │
  │   ├── api/                       # All API endpoints
  │   │   ├── mod.rs
  │   │   ├── grpc/                  # gRPC services
  │   │   │   ├── mod.rs
  │   │   │   ├── prover.rs          # ProverNetwork service handlers
  │   │   │   ├── artifact.rs        # ArtifactStore service handlers
  │   │   │   └── program.rs         # Program-specific handlers 
  │   │   ├── http.rs                # HTTP handlers 
  │   │   └── websocket.rs           # WebSocket handlers 
  │   │
  │   ├── worker/                    # Worker management 
  │   │   ├── mod.rs
  │   │   ├── registry.rs            # Worker registration & tracking 
  │   │   ├── health.rs              # Heartbeat & health monitoring 
  │   │   ├── assignment.rs          # Proof assignment logic 
  │   │   └── communication.rs       # Worker messaging 
  │   │
  │   ├── proof/                     # Proof management
  │   │   ├── mod.rs
  │   │   ├── router.rs              # Proof routing logic 
  │   │   ├── status.rs              # Status tracking 
  │   │   ├── history.rs             # Completed proof tracking 
  │   │   └── queue.rs               # Request queue management 
  │   │
  │   ├── storage/                   # Storage layer
  │   │   ├── mod.rs
  │   │   ├── artifact.rs            # Artifact metadata & URIs 
  │   │   ├── filesystem.rs          # File operations 
  │   │   └── lifecycle.rs           # Cleanup & rotation 
  │   │
  │   └── auth/                      # Authentication & authorization
  │       ├── mod.rs
  │       ├── nonce.rs               # Nonce management 
  │       └── signature.rs           # Signature verification 
```

## `network-lib` overview

```bash
  network-lib/
  ├── Cargo.toml
  └── src/
      ├── lib.rs                     # Core protocol definitions
      ├── messages.rs                # Message types 
      ├── protocol.rs                # Protocol constants
      └── serialization.rs           # Bincode helpers
```
