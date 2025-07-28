#!/bin/sh

# Write the config.
# TODO: switch to .env config
cd ~
cat >~/magister.toml <<EOF
# both hierophant and magister are being run on the same machine
this_magister_addr = "http://${THIS_IP}"
hierophant_ip = "${THIS_IP}"
hierophant_http_port = ${PORT__HIEROPHANT_HTTP}
http_port=${PORT__MAGISTER_HTTP}
vast_api_key="${VAST_API_KEY}"

number_instances=${NUMBER_INSTANCES}

# Contemplant vast.ai template
template_hash = "${VAST_TEMPLATE_HASH}"
# vast hosts who have been unreliable
bad_hosts = [213498, 74292, 113132, 249296]
bad_machines = [12217,19571]
# vast hosts who have been reliable
good_hosts = [207289, 1276]
good_machines = [13428, 8218]

[vast_query]
allocated_storage = 16
gpu_name = "RTX 4090"
reliability = 0.99
min_cuda_version = 12.8
gpu_ram = 21
disk_space = 16
duration = 192679
cost_per_hour = 0.6

EOF

echo "Starting Magister Vast.ai instance manager..."
exec /usr/local/bin/magister "$@"
