# Simple Makefile for Hierophant
.PHONY: hierophant contemplant 

check-env:
	@test -f .env || (echo "Error: .env file not found!" && exit 1)

# Start in foreground
prover-network: check-env
	docker compose -f docker-compose.yml up

# Start in background
prover-network-d: check-env
	docker compose -f docker-compose.yml up -d

# Stop 
stop-prover-network:
	docker compose -f docker-compose.yml down

# View  logs
logs-prover-network:
	docker compose -f docker-compose.yml logs -f

# Restart hierophant
restart-prover-network:
	docker compose -f docker-compose.yml restart

# Run just the hierophant
hierophant:
	@test -f hierophant.toml || (echo "Error: hierophant.toml not found" && exit 1)
	RUST_LOG=info RUST_BACKTRACE=1 cargo run --release --bin hierophant

# Run just the contemplant
contemplant:
	@test -f contemplant.toml || (echo "Error: contemplant.toml not found" && exit 1)
	RUST_LOG=info RUST_BACKTRACE=1 cargo run --release --bin contemplant

