# Prover Vendor Assets

This directory holds the SHA256 checksums for all large binary assets needed
to build contemplant images that support each ZK VM. The actual tarballs
(several GB combined) are not stored in git; they live on the vendor CDN at
`${VENDOR_BASE_URL}`, and the Docker builds download + verify each one
against the committed `.sha256` file here.

## Directory Structure

Assets are namespaced by VM and versioned by their upstream tag:

```
provers/
├── sp1/
│   └── <SP1_CIRCUITS_VERSION>/      # e.g. v5.0.0 (sp1-sdk crate version)
│       ├── groth16.tar.gz.sha256    # SP1 Groth16 circuit artifacts
│       ├── plonk.tar.gz.sha256      # SP1 Plonk circuit artifacts
│       └── moongate-server.tar.gz.sha256  # Succinct CUDA accelerator
└── risc0/
    └── <RISC0_GROTH16_PROVER_TAG>/  # e.g. v2025-04-03.1 (upstream tag)
        └── risc0-groth16-prover.tar.gz.sha256  # gnark prover + keys
```

The matching layout on the vendor CDN is identical:

```
${VENDOR_BASE_URL}/
├── sp1/<SP1_CIRCUITS_VERSION>/{groth16,plonk,moongate-server}.tar.gz
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

2. Produce the three tarballs locally:

   ```bash
   # Circuits; let SP1 SDK download them by running any SP1 program, then tar
   cd ~/.sp1/circuits/groth16 && tar -czf /tmp/groth16.tar.gz <new-version>/
   cd ~/.sp1/circuits/plonk && tar -czf /tmp/plonk.tar.gz <new-version>/

   # moongate-server; extract from Succinct's CUDA prover docker image
   # (image + tag depend on the SP1 release; see Succinct's release notes)
   docker create --name moongate-extract succinctlabs/moongate-server:<tag>
   docker cp \
     moongate-extract:/usr/local/bin/moongate-server \
     /tmp/moongate-server
   docker rm moongate-extract
   tar -czf /tmp/moongate-server.tar.gz -C /tmp moongate-server
   ```

3. Generate checksums and commit them:

   ```bash
   cd /tmp
   for f in groth16.tar.gz plonk.tar.gz moongate-server.tar.gz; do
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
