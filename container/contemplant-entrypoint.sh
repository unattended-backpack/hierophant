#!/bin/bash
set -e

# Set tmux to use user-writable socket directory
export TMUX_TMPDIR=$HOME/.tmux
mkdir -p $TMUX_TMPDIR

# Generate SSH host keys in user-writable location. Skip regeneration if keys
# already exist; keeps the entrypoint idempotent across container restarts or
# when docker-compose reuses a container whose writable layer survived a prior
# run (which would otherwise cause ssh-keygen to prompt interactively and hang).
[ -f ~/ssh_host_keys/ssh_host_rsa_key ]     || ssh-keygen -t rsa     -f ~/ssh_host_keys/ssh_host_rsa_key     -N '' -q
[ -f ~/ssh_host_keys/ssh_host_ecdsa_key ]   || ssh-keygen -t ecdsa   -f ~/ssh_host_keys/ssh_host_ecdsa_key   -N '' -q
[ -f ~/ssh_host_keys/ssh_host_ed25519_key ] || ssh-keygen -t ed25519 -f ~/ssh_host_keys/ssh_host_ed25519_key -N '' -q

# Write SSH authorized keys from environment if provided
if [ -n "$SSH_AUTHORIZED_KEYS" ]; then
  echo "Setting up SSH authorized keys from environment..."
  echo "$SSH_AUTHORIZED_KEYS" > ~/.ssh/authorized_keys
  chmod 600 ~/.ssh/authorized_keys
else
  echo "No SSH_AUTHORIZED_KEYS provided, SSH access disabled"
fi

# Build environment variable exports for tmux sessions (only include set variables).
# The contemplant binary itself reads these via env-var overrides in config.rs
# (or via the mounted contemplant.toml if present).
#
# No prover-server lifecycle lives here anymore: sp1-sdk 6.x's CUDA path has
# the contemplant itself spawn `sp1-gpu-server` (vendored into the image at
# ~/.sp1/bin/ by Dockerfile.contemplant) and talk to it over a unix socket.
# The spawned server inherits the contemplant's stdio, so its output lands in
# contemplant-output.txt with everything else - no separate log tail needed.
ENV_EXPORTS=""
[ -n "$HIEROPHANT_WS_ADDRESS" ] && ENV_EXPORTS="$ENV_EXPORTS export HIEROPHANT_WS_ADDRESS=\"$HIEROPHANT_WS_ADDRESS\";"
[ -n "$MAGISTER_DROP_ENDPOINT" ] && ENV_EXPORTS="$ENV_EXPORTS export MAGISTER_DROP_ENDPOINT=\"$MAGISTER_DROP_ENDPOINT\";"
[ -n "$CONTEMPLANT_NAME" ] && ENV_EXPORTS="$ENV_EXPORTS export CONTEMPLANT_NAME=\"$CONTEMPLANT_NAME\";"
[ -n "$HTTP_PORT" ] && ENV_EXPORTS="$ENV_EXPORTS export HTTP_PORT=\"$HTTP_PORT\";"
[ -n "$CONTEMPLANT_VMS" ] && ENV_EXPORTS="$ENV_EXPORTS export CONTEMPLANT_VMS=\"$CONTEMPLANT_VMS\";"
[ -n "$CONTEMPLANT_SP1_BACKEND" ] && ENV_EXPORTS="$ENV_EXPORTS export CONTEMPLANT_SP1_BACKEND=\"$CONTEMPLANT_SP1_BACKEND\";"
[ -n "$CONTEMPLANT_RISC0_BACKEND" ] && ENV_EXPORTS="$ENV_EXPORTS export CONTEMPLANT_RISC0_BACKEND=\"$CONTEMPLANT_RISC0_BACKEND\";"
[ -n "$CONTEMPLANT_RISC0_GROTH16" ] && ENV_EXPORTS="$ENV_EXPORTS export CONTEMPLANT_RISC0_GROTH16=\"$CONTEMPLANT_RISC0_GROTH16\";"
[ -n "$HEARTBEAT_INTERVAL_SECONDS" ] && ENV_EXPORTS="$ENV_EXPORTS export HEARTBEAT_INTERVAL_SECONDS=\"$HEARTBEAT_INTERVAL_SECONDS\";"
[ -n "$MAX_PROOFS_STORED" ] && ENV_EXPORTS="$ENV_EXPORTS export MAX_PROOFS_STORED=\"$MAX_PROOFS_STORED\";"
[ -n "$WATCHER_POLLING_INTERVAL_MS" ] && ENV_EXPORTS="$ENV_EXPORTS export WATCHER_POLLING_INTERVAL_MS=\"$WATCHER_POLLING_INTERVAL_MS\";"

# The contemplant binary links libcuda (the CUDA driver library) eagerly
# when built with the GPU features. On GPU-granted containers,
# nvidia-container-runtime injects the real driver and registers it with
# the loader; without a GPU nothing provides libcuda and the binary cannot
# load. Detect the real driver's absence and fall back to the baked NVIDIA
# stub - symbol-complete but inert - so the same universal image starts on
# CPU-only hosts. Never add the stub when a real driver exists: the stub
# path would need to lose the resolution race, and there is no reason to
# run it.
if ! ldconfig -p 2>/dev/null | grep -q 'libcuda\.so\.1'; then
  echo "[entrypoint] No CUDA driver present; using inert libcuda stub (CPU-only operation)"
  export LD_LIBRARY_PATH="/opt/cuda-stubs${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  ENV_EXPORTS="$ENV_EXPORTS export LD_LIBRARY_PATH=\"$LD_LIBRARY_PATH\";"
fi

# Unset TMUX variable if we're running inside an existing tmux session
# This ensures we create a new independent tmux server
unset TMUX

tmux new-session -d -s contemplant
# Startup probe for the risc0-groth16 docker shim. risc0-groth16 3.x checks
# `docker --version` before attempting the snark wrap; if this probe fails
# here we know immediately that the shim isn't reachable and can abort with a
# clear message instead of discovering it 30 seconds into a proof.
echo "[entrypoint] Probing risc0-groth16 docker shim..."
echo "[entrypoint]   PATH=$PATH"
echo "[entrypoint]   which docker: $(which docker 2>&1 || echo '<not found>')"
echo "[entrypoint]   ls -la /usr/local/bin/docker:"
ls -la /usr/local/bin/docker 2>&1 | sed 's/^/[entrypoint]   /'
echo "[entrypoint]   docker --version output:"
# Capture docker's exit code separately; piping straight to sed would mask
# the real exit behind sed's (always-0) exit. A failing shim here predicts
# Groth16 jobs will abort with 'Please install docker first.'
docker_out=$(docker --version 2>&1)
docker_rc=$?
printf '%s\n' "$docker_out" | sed 's/^/[entrypoint]   /'
if [ "$docker_rc" -eq 0 ]; then
  echo "[entrypoint]   docker --version exit: 0 (shim responding)"
else
  echo "[entrypoint]   docker --version exit: $docker_rc"
  echo "[entrypoint] WARNING: Groth16 proofs will fail with 'Please install docker first.' until this is fixed."
fi

tmux new-window -t contemplant -n contemplant "bash -c '$ENV_EXPORTS RUST_LOG=info RUST_LOG_STYLE=always RUST_BACKTRACE=1 contemplant 2>&1 | tee contemplant-output.txt'"

# Forward the contemplant log file's tail to our own stdout so docker-compose
# logs / `docker logs` surface contemplant output. Without this, tee inside
# tmux traps output in the tmux pty and PID 1 only emits supervisord/sshd
# lines; which makes a contemplant crash invisible (symptom we hit while
# debugging Groth16-direct: silent contemplant + ws reset + no diagnostic).
(
  until [ -f /home/contemplant/contemplant-output.txt ]; do sleep 0.2; done
  tail -F /home/contemplant/contemplant-output.txt 2>/dev/null &
) &

# Start supervisord to launch SSH (runs in foreground with -n).
exec /usr/bin/supervisord -c ~/supervisord.conf -n
