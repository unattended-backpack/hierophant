#!/bin/bash
set -e

# Log environment
# websocket address in the form of <ws://127.0.0.1:3000/ws> (where port is http server)
echo "HIEROPHANT_WS_ADDRESS=$HIEROPHANT_WS_ADDRESS"
# endpoint for dropping this contemplant
echo "MAGISTER_DROP_ENDPOINT=$MAGISTER_DROP_ENDPOINT"

# Write the dynamic config.
cd ~
cat >~/contemplant.toml <<EOF
hierophant_ws_address = "${HIEROPHANT_WS_ADDRESS}"
moongate_endpoint = "http://localhost:3000/twirp/"
$(if [ -n "$MAGISTER_DROP_ENDPOINT" ]; then echo "magister_drop_endpoint = \"${MAGISTER_DROP_ENDPOINT}\""; fi)

EOF

# Generate SSH keys.
ssh-keygen -A

# Set up the tmux session.
tmux new-session -d -s contemplant "bash -c 'RUST_LOG=info RUST_LOG_STYLE=always RUST_BACKTRACE=1 moongate-server 2>&1 | tee moongate.log'"
sleep 30
tmux new-window -t contemplant -n contemplant "bash -c 'RUST_LOG=info RUST_LOG_STYLE=always RUST_BACKTRACE=1 contemplant 2>&1 | tee contemplant-output.txt'"

# Start supervisor to launch SSH.
exec /usr/bin/supervisord -c /etc/supervisor/conf.d/supervisord.conf -n
