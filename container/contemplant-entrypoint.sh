#!/bin/bash
set -e

# Generate SSH host keys in user-writable location.
ssh-keygen -t rsa -f ~/ssh_host_keys/ssh_host_rsa_key -N '' -q
ssh-keygen -t ecdsa -f ~/ssh_host_keys/ssh_host_ecdsa_key -N '' -q
ssh-keygen -t ed25519 -f ~/ssh_host_keys/ssh_host_ed25519_key -N '' -q

# Read config from contemplant.toml if not set in environment.
if [ -z "$PROVER_TYPE" ] && [ -f contemplant.toml ]; then
  PROVER_TYPE=$(grep -E '^\s*prover_type\s*=' contemplant.toml | sed -E 's/.*=\s*"?([^"]+)"?.*/\1/' || echo "cpu")
fi
PROVER_TYPE=${PROVER_TYPE:-cpu}
if [ -z "$MOONGATE_ENDPOINT" ] && [ -f contemplant.toml ]; then
  MOONGATE_ENDPOINT=$(grep -E '^\s*moongate_endpoint\s*=' contemplant.toml | sed -E 's/.*=\s*"([^"]+)".*/\1/' || echo "")
fi

# Only start moongate-server if using CUDA with a moongate endpoint.
if [ "$PROVER_TYPE" = "cuda" ] && [ -n "$MOONGATE_ENDPOINT" ]; then
  echo "Starting moongate-server (MOONGATE_ENDPOINT=$MOONGATE_ENDPOINT)"
  tmux new-session -d -s contemplant -n moongate "bash -c 'RUST_LOG=info RUST_LOG_STYLE=always RUST_BACKTRACE=1 moongate-server 2>&1 | tee moongate.log'"
  sleep 30
else
  echo "Skipping moongate-server (PROVER_TYPE=$PROVER_TYPE, MOONGATE_ENDPOINT=${MOONGATE_ENDPOINT:-not set})"
  tmux new-session -d -s contemplant
fi
tmux new-window -t contemplant -n contemplant "bash -c 'RUST_LOG=info RUST_LOG_STYLE=always RUST_BACKTRACE=1 contemplant 2>&1 | tee contemplant-output.txt'"

# Start supervisord to launch SSH (runs in foreground with -n).
exec /usr/bin/supervisord -c ~/supervisord.conf -n
