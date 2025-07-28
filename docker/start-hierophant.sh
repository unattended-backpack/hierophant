#!/bin/sh

# Write the config.
# TODO: switch to .env config
cd ~
cat >~/hierophant.toml <<EOF
# only used for artifact upload
this_hierophant_ip = "${THIS_IP}"
grpc_port=${PORT__HIEROPHANT_GRPC}
http_port=${PORT__HIEROPHANT_HTTP}

EOF

echo "Starting Hierophant prover network..."
exec /usr/local/bin/hierophant "$@"
