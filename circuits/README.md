# Circuit Artifacts

This directory contains SHA256 checksums for SP1 circuit artifacts (Groth16 and Plonk).
The actual circuit files (~4GB) are not stored in git. They are downloaded and verified during Docker builds.

## Directory Structure

Circuits are versioned to match SP1 SDK versions:
- `v5.0.0/groth16/` - Groth16 circuit checksums for SP1 v5.0.0
- `v5.0.0/plonk/` - Plonk circuit checksums for SP1 v5.0.0

## Upgrading to a New SP1 Version

When upgrading to a new SP1 version (e.g., v5.0.0 → v5.1.0):

1. **Download the new circuits** (let SP1 SDK download them):
   ```bash
   # Update sp1-sdk version in Cargo.toml
   # Run any SP1 program - circuits download to ~/.sp1/circuits/v5.1.0/
   ```

2. **Create new checksums directory**:
   ```bash
   mkdir -p circuits/v5.1.0/groth16 circuits/v5.1.0/plonk
   ```

3. **Create compressed archives** for the new circuit files:
   ```bash
   # Create groth16 archive
   cd ~/.sp1/circuits/groth16
   tar -czf /tmp/groth16.tar.gz v5.1.0/

   # Create plonk archive
   cd ~/.sp1/circuits/plonk
   tar -czf /tmp/plonk.tar.gz v5.1.0/
   ```

4. **Generate checksums** for the archives:
   ```bash
   # Create checksum directories
   mkdir -p /path/to/hierophant/circuits/v5.1.0/groth16
   mkdir -p /path/to/hierophant/circuits/v5.1.0/plonk

   # Generate checksums (with just the filename, not full path)
   cd /tmp
   sha256sum groth16.tar.gz | sed 's|/tmp/||' > /path/to/hierophant/circuits/v5.1.0/groth16/groth16.tar.gz.sha256
   sha256sum plonk.tar.gz | sed 's|/tmp/||' > /path/to/hierophant/circuits/v5.1.0/plonk/plonk.tar.gz.sha256
   ```

5. **Upload to vendor server** under versioned path:
   ```bash
   # Upload to: ${VENDOR_BASE_URL}/v5.1.0/groth16.tar.gz
   #            ${VENDOR_BASE_URL}/v5.1.0/plonk.tar.gz
   ```

5. **Update default version** in `Dockerfile.hierophant.ci`:
   ```dockerfile
   ARG CIRCUITS_VERSION=v5.1.0  # Change from v5.0.0
   ```

6. **Test the build**:
   ```bash
   make ci  # Should download v5.1.0 circuits
   ```
