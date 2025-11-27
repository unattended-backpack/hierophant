#!/bin/bash
set -e

# Set tmux to use user-writable socket directory
export TMUX_TMPDIR=$HOME/.tmux
mkdir -p $TMUX_TMPDIR

# Generate SSH host keys in user-writable location.
ssh-keygen -t rsa -f ~/ssh_host_keys/ssh_host_rsa_key -N '' -q
ssh-keygen -t ecdsa -f ~/ssh_host_keys/ssh_host_ecdsa_key -N '' -q
ssh-keygen -t ed25519 -f ~/ssh_host_keys/ssh_host_ed25519_key -N '' -q

# Write SSH authorized keys from environment if provided
if [ -n "$SSH_AUTHORIZED_KEYS" ]; then
  echo "Setting up SSH authorized keys from environment..."
  echo "$SSH_AUTHORIZED_KEYS" > ~/.ssh/authorized_keys
  chmod 600 ~/.ssh/authorized_keys
else
  echo "No SSH_AUTHORIZED_KEYS provided, SSH access disabled"
fi

# Read config from contemplant.toml if not set in environment.
if [ -z "$PROVER_TYPE" ] && [ -f contemplant.toml ]; then
  PROVER_TYPE=$(grep -E '^\s*prover_type\s*=' contemplant.toml | sed -E 's/.*=\s*"?([^"]+)"?.*/\1/' || echo "cpu")
fi
PROVER_TYPE=${PROVER_TYPE:-cpu}
if [ -z "$MOONGATE_ENDPOINT" ] && [ -f contemplant.toml ]; then
  MOONGATE_ENDPOINT=$(grep -E '^\s*moongate_endpoint\s*=' contemplant.toml | sed -E 's/.*=\s*"([^"]+)".*/\1/' || echo "")
fi

# Build environment variable exports for tmux sessions (only include set variables)
ENV_EXPORTS=""
[ -n "$HIEROPHANT_WS_ADDRESS" ] && ENV_EXPORTS="$ENV_EXPORTS export HIEROPHANT_WS_ADDRESS=\"$HIEROPHANT_WS_ADDRESS\";"
[ -n "$MAGISTER_DROP_ENDPOINT" ] && ENV_EXPORTS="$ENV_EXPORTS export MAGISTER_DROP_ENDPOINT=\"$MAGISTER_DROP_ENDPOINT\";"
[ -n "$PROVER_TYPE" ] && ENV_EXPORTS="$ENV_EXPORTS export PROVER_TYPE=\"$PROVER_TYPE\";"
[ -n "$CONTEMPLANT_NAME" ] && ENV_EXPORTS="$ENV_EXPORTS export CONTEMPLANT_NAME=\"$CONTEMPLANT_NAME\";"
[ -n "$HTTP_PORT" ] && ENV_EXPORTS="$ENV_EXPORTS export HTTP_PORT=\"$HTTP_PORT\";"
[ -n "$MOONGATE_ENDPOINT" ] && ENV_EXPORTS="$ENV_EXPORTS export MOONGATE_ENDPOINT=\"$MOONGATE_ENDPOINT\";"
[ -n "$HEARTBEAT_INTERVAL_SECONDS" ] && ENV_EXPORTS="$ENV_EXPORTS export HEARTBEAT_INTERVAL_SECONDS=\"$HEARTBEAT_INTERVAL_SECONDS\";"
[ -n "$MAX_PROOFS_STORED" ] && ENV_EXPORTS="$ENV_EXPORTS export MAX_PROOFS_STORED=\"$MAX_PROOFS_STORED\";"
[ -n "$MOONGATE_LOG_PATH" ] && ENV_EXPORTS="$ENV_EXPORTS export MOONGATE_LOG_PATH=\"$MOONGATE_LOG_PATH\";"
[ -n "$WATCHER_POLLING_INTERVAL_MS" ] && ENV_EXPORTS="$ENV_EXPORTS export WATCHER_POLLING_INTERVAL_MS=\"$WATCHER_POLLING_INTERVAL_MS\";"

# Unset TMUX variable if we're running inside an existing tmux session
# This ensures we create a new independent tmux server
unset TMUX

# Only start moongate-server if using CUDA with a moongate endpoint.
if [ "$PROVER_TYPE" = "cuda" ] && [ -n "$MOONGATE_ENDPOINT" ]; then
  echo "Starting moongate-server (MOONGATE_ENDPOINT=$MOONGATE_ENDPOINT)"
  tmux new-session -d -s contemplant -n moongate "bash -c '$ENV_EXPORTS RUST_LOG=info RUST_LOG_STYLE=always RUST_BACKTRACE=1 moongate-server 2>&1 | tee moongate.log'"
  sleep 30
else
  echo "Skipping moongate-server (PROVER_TYPE=$PROVER_TYPE, MOONGATE_ENDPOINT=${MOONGATE_ENDPOINT:-not set})"
  tmux new-session -d -s contemplant
fi
tmux new-window -t contemplant -n contemplant "bash -c '$ENV_EXPORTS RUST_LOG=info RUST_LOG_STYLE=always RUST_BACKTRACE=1 contemplant 2>&1 | tee contemplant-output.txt'"

# Start supervisord to launch SSH (runs in foreground with -n).
exec /usr/bin/supervisord -c ~/supervisord.conf -n
