# Prover Vendor Assets

This directory holds the SHA256 checksums for all large binary assets needed
to build contemplant images that support each ZK VM. The actual tarballs
(several GB combined) are not stored in git; they live on the vendor CDN at
`${VENDOR_BASE_URL}`, and the Docker builds download then verify each one
against the committed `.sha256` file here.

## Directory Structure

Assets are namespaced by VM and versioned by their upstream tag:

```
provers/
├── sp1/
│   ├── <SP1_CIRCUITS_VERSION>/      # e.g. v6.1.0 (SP1_CIRCUIT_VERSION const)
│   │   ├── groth16.tar.gz.sha256    # SP1 Groth16 circuit artifacts
│   │   └── plonk.tar.gz.sha256      # SP1 Plonk circuit artifacts
│   └── gpu-server/<SP1_GPU_SERVER_VERSION>/  # keyed by the sp1-sdk crate version
│       └── sp1-gpu-server.tar.gz.sha256  # Succinct CUDA prover server
├── risc0/
│   └── <RISC0_GROTH16_PROVER_TAG>/  # e.g. v2025-04-03.1 (upstream tag)
│       └── risc0-groth16-prover.tar.gz.sha256  # gnark prover + keys
└── openvm/                          # see openvm/README.md
```

The matching layout on the vendor CDN is identical:

```
${VENDOR_BASE_URL}/
├── sp1/<SP1_CIRCUITS_VERSION>/{groth16,plonk}.tar.gz
├── sp1/gpu-server/<SP1_GPU_SERVER_VERSION>/sp1-gpu-server.tar.gz
└── risc0/<RISC0_GROTH16_PROVER_TAG>/risc0-groth16-prover.tar.gz
```

## Adding support for a new SP1 version

When bumping `sp1-sdk` (e.g. v5.0.0 → v5.2.0), you need new versions of all
three SP1 vendor assets. The `.env.maintainer` variable that drives this is
`SP1_CIRCUITS_VERSION`.

1. Create the version's checksum directory:

   ```bash
   mkdir -p provers/sp1/<new-version>
   ```

2. Produce the three tarballs locally. `<new-version>` is the
   `SP1_CIRCUIT_VERSION` constant baked into the sp1-prover crate you are
   bumping to (NOT necessarily the sdk version; e.g. every sdk from 6.2.1
   through 6.5.0 pins circuits v6.1.0):

   ```bash
   # Circuits; download Succinct's release artifacts directly and re-wrap
   # them with the <new-version>/ top-level directory the Dockerfiles expect
   # (strip macOS ._AppleDouble entries; upstream packages carry them):
   for kind in groth16 plonk; do
     curl -fsSL -o "$kind-upstream.tar.gz" \
       "https://sp1-circuits.s3-us-east-2.amazonaws.com/<new-version>-$kind.tar.gz"
     mkdir -p "$kind/<new-version>"
     tar -xzf "$kind-upstream.tar.gz" -C "$kind/<new-version>"
     find "$kind" -name '._*' -delete
     tar -C "$kind" -czf "/tmp/$kind.tar.gz" <new-version>
   done

   # sp1-gpu-server; from the SP1 GitHub release matching the SDK version
   # (the runtime `--version`-checks the binary against the sdk's crate
   # version and re-downloads on mismatch, so these must agree exactly):
   curl -fsSL -o gpu.tar.gz \
     "https://github.com/succinctlabs/sp1/releases/download/v<sdk-version>/sp1_gpu_server_v<sdk-version>_x86_64.tar.gz"
   mkdir gpu && tar -xzf gpu.tar.gz -C gpu
   tar -C gpu -czf /tmp/sp1-gpu-server.tar.gz sp1-gpu-server
   ```

3. Generate checksums and commit them:

   ```bash
   cd /tmp
   for f in groth16.tar.gz plonk.tar.gz sp1-gpu-server.tar.gz; do
     sha256sum "$f" > <repo>/provers/sp1/<new-version>/"$f".sha256
   done
   ```

4. Upload the tarballs to `${VENDOR_BASE_URL}/sp1/<new-version>/`.

5. Bump `SP1_CIRCUITS_VERSION=<new-version>` in `.env.maintainer`.

6. `make test-sp1` to verify.

## Adding support for a new RISC Zero groth16-prover version

When `risc0-zkvm` is upgraded and the new version pins a newer docker image
tag (see `risc0-groth16-3.x/src/prove/docker.rs`), re-extract and re-vendor.

1. Pull the new image and create the checksum directory:

   ```bash
   docker pull risczero/risc0-groth16-prover:<new-tag>
   mkdir -p provers/risc0/<new-tag>
   ```

2. Extract the 5 assets into a flat tarball (see the shim at
   `container/risc0-groth16-shim/docker` for the expected layout):

   ```bash
   docker create --name r0g16-extract risczero/risc0-groth16-prover:<new-tag>
   mkdir /tmp/r0g16-assets
   for f in /app/stark_verify.cs /app/stark_verify.dat \
            /app/stark_verify_final.pk.dmp \
            /usr/local/bin/stark_verify /usr/local/bin/prover; do
     docker cp r0g16-extract:"$f" /tmp/r0g16-assets/
   done
   docker rm r0g16-extract
   tar -C /tmp/r0g16-assets -czf /tmp/risc0-groth16-prover.tar.gz .
   ```

3. Commit the checksum:

   ```bash
   cd /tmp
   sha256sum risc0-groth16-prover.tar.gz \
     > <repo>/provers/risc0/<new-tag>/risc0-groth16-prover.tar.gz.sha256
   ```

4. Upload the tarball to `${VENDOR_BASE_URL}/risc0/<new-tag>/`.

5. Bump `RISC0_GROTH16_PROVER_TAG=<new-tag>` in `.env.maintainer`.

6. `make test-risc0` to verify.
