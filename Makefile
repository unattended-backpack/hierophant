# Configuration is loaded from `.env.maintainer` and can be overridden by
# environment variables.
#
# Usage:
#   make build                    # Build using `.env.maintainer`.
#   BUILD_IMAGE=... make build    # Override specific variables.

# Load configuration from `.env.maintainer` if it exists.
-include .env.maintainer

# Load configuration from `.env` if it exists.
-include .env

# Allow environment variable overrides with defaults.
BUILD_IMAGE ?= unattended/petros:latest
RUNTIME_IMAGE ?= debian:trixie-slim
VENDOR_BASE_URL ?=
CIRCUITS_VERSION ?=
DOCKER_BUILD_ARGS ?=
DOCKER_RUN_ARGS ?=
HIEROPHANT_NAME ?= hierophant
CONTEMPLANT_NAME ?= contemplant
IMAGE_TAG ?= latest
ACT_PULL ?= true

.PHONY: init
init:
	@echo "Initializing configuration files ..."
	@if [ ! -f .env ]; then \
		cp .env.example .env; \
		echo "Created .env from .env.example."; \
	else \
		echo ".env already exists."; \
	fi
	@if [ ! -f hierophant.toml ]; then \
		cp hierophant.example.toml hierophant.toml; \
		echo "Created hierophant.toml from hierophant.example.toml."; \
	else \
		echo "hierophant.toml already exists."; \
	fi
	@if [ ! -f contemplant.toml ]; then \
		cp contemplant.example.toml contemplant.toml; \
		echo "Created contemplant.toml from contemplant.example.toml."; \
	else \
		echo "contemplant.toml already exists."; \
	fi
	@echo "Initialization complete. Review configuration before running."

.PHONY: circuits
circuits:
	@echo "Downloading and verifying circuit artifacts ..."
	@if [ -z "$(VENDOR_BASE_URL)" ] || [ -z "$(CIRCUITS_VERSION)" ]; then \
		echo "ERROR: VENDOR_BASE_URL and CIRCUITS_VERSION must be set" >&2; \
		echo "Load them from .env.maintainer or set as environment variables" >&2; \
		exit 1; \
	fi
	@echo "  Vendor URL: $(VENDOR_BASE_URL)"
	@echo "  Circuits version: $(CIRCUITS_VERSION)"
	@mkdir -p ~/.sp1/circuits/groth16/$(CIRCUITS_VERSION)
	@mkdir -p ~/.sp1/circuits/plonk/$(CIRCUITS_VERSION)
	@echo "Downloading groth16 circuits ..."
	@VENDOR_BASE_URL=$(VENDOR_BASE_URL) container/vendor.sh "groth16.tar.gz" "circuits/$(CIRCUITS_VERSION)/groth16" "$(CIRCUITS_VERSION)/"
	@echo "Downloading plonk circuits ..."
	@VENDOR_BASE_URL=$(VENDOR_BASE_URL) container/vendor.sh "plonk.tar.gz" "circuits/$(CIRCUITS_VERSION)/plonk" "$(CIRCUITS_VERSION)/"
	@echo "Installing to ~/.sp1/circuits/ ..."
	@cp -r /tmp/extracted-groth16/$(CIRCUITS_VERSION)/* ~/.sp1/circuits/groth16/$(CIRCUITS_VERSION)/
	@cp -r /tmp/extracted-plonk/$(CIRCUITS_VERSION)/* ~/.sp1/circuits/plonk/$(CIRCUITS_VERSION)/
	@touch ~/.sp1/circuits/groth16/.download_complete
	@touch ~/.sp1/circuits/plonk/.download_complete
	@rm -rf /tmp/extracted-groth16 /tmp/extracted-plonk
	@echo "Circuit artifacts installed successfully."

.PHONY: moongate
moongate:
	@echo "Downloading and verifying moongate-server ..."
	@if [ -z "$(VENDOR_BASE_URL)" ]; then \
		echo "ERROR: VENDOR_BASE_URL must be set" >&2; \
		echo "Load it from .env.maintainer or set as environment variable" >&2; \
		exit 1; \
	fi
	@echo "  Vendor URL: $(VENDOR_BASE_URL)"
	@echo "Downloading moongate-server ..."
	@VENDOR_BASE_URL=$(VENDOR_BASE_URL) container/vendor.sh "moongate-server.tar.gz" "container" ""
	@echo "Installing to /usr/local/bin/ ..."
	@sudo cp /tmp/extracted-moongate-server/moongate-server /usr/local/bin/moongate-server
	@sudo chmod +x /usr/local/bin/moongate-server
	@rm -rf /tmp/extracted-moongate-server
	@echo "moongate-server installed successfully to /usr/local/bin/moongate-server."

.PHONY: clean
clean:
	@bash -c 'echo -e "\033[33mWARNING: This will delete build artifacts.\033[0m"; \
	read -p "Are you sure you want to continue? [y/N]: " confirm; \
	if [[ "$$confirm" != "y" && "$$confirm" != "Y" ]]; then \
		echo "Operation cancelled."; \
		exit 1; \
	fi'
	rm -rf out/
	rm -rf target/
	rm -f result result-*

.PHONY: build
build:
	@echo "Building native artifacts ..."
	mkdir -p out
	cargo build --release --bin hierophant
	cargo build --release --bin contemplant --features enable-native-gnark
	cp ./target/release/hierophant ./out/hierophant
	cp ./target/release/contemplant ./out/contemplant
	@echo "Build complete."

.PHONY: test
test:
	@echo "Running tests ..."
	cargo test --release
	@echo "... tests completed."

.PHONY: integration
integration:
	$(MAKE) build
	$(MAKE) ci
	@echo "Running integration test ..."
	@echo "Starting Hierophant, Contemplant, and test client ..."
	docker-compose -f docker-compose.test.yml up \
		--build \
		--abort-on-container-exit \
		--exit-code-from test-client
	@echo "Cleaning up containers ..."
	docker-compose -f docker-compose.test.yml down -v
	@echo "Integration test complete."

.PHONY: docker-h
docker-h:
	@echo "Building Hierophant image ..."
	@echo "  Build image:   $(BUILD_IMAGE)"
	@echo "  Runtime image: $(RUNTIME_IMAGE)"
	@echo "  Output tag:    $(HIEROPHANT_NAME):$(IMAGE_TAG)"
	@mkdir -p out
	docker build \
		$(DOCKER_BUILD_ARGS) \
		--build-arg BUILD_IMAGE=$(BUILD_IMAGE) \
		--build-arg RUNTIME_IMAGE=$(RUNTIME_IMAGE) \
		--build-arg VENDOR_BASE_URL=$(VENDOR_BASE_URL) \
		--build-arg CIRCUITS_VERSION=$(CIRCUITS_VERSION) \
		-f Dockerfile.hierophant \
		-t $(HIEROPHANT_NAME):$(IMAGE_TAG) \
		.
	@echo "Build complete: $(HIEROPHANT_NAME):$(IMAGE_TAG)"

.PHONY: docker-c
docker-c:
	@echo "Building Contemplant image ..."
	@echo "  Build image:   $(BUILD_IMAGE)"
	@echo "  Runtime image: $(RUNTIME_IMAGE)"
	@echo "  Output tag:    $(CONTEMPLANT_NAME):$(IMAGE_TAG)"
	@mkdir -p out
	docker build \
		$(DOCKER_BUILD_ARGS) \
		--build-arg BUILD_IMAGE=$(BUILD_IMAGE) \
		--build-arg RUNTIME_IMAGE=$(RUNTIME_IMAGE) \
		--build-arg VENDOR_BASE_URL=$(VENDOR_BASE_URL) \
		--build-arg CIRCUITS_VERSION=$(CIRCUITS_VERSION) \
		-f Dockerfile.contemplant \
		-t $(CONTEMPLANT_NAME):$(IMAGE_TAG) \
		.
	@echo "Build complete: $(CONTEMPLANT_NAME):$(IMAGE_TAG)"

.PHONY: docker
docker:
	$(MAKE) docker-h
	$(MAKE) docker-c

.PHONY: ci
ci:
	@echo "Building Docker images from pre-built binaries (CI mode) ..."
	@if [ ! -f out/hierophant ] || [ ! -f out/contemplant ]; then \
		echo "ERROR: Pre-built binaries not found in ./out/" >&2; \
		echo "Run 'make build' first to create the binaries." >&2; \
		exit 1; \
	fi
	@echo "  Build image:   $(BUILD_IMAGE)"
	@echo "  Runtime image: $(RUNTIME_IMAGE)"
	@echo "  Output tag:    $(HIEROPHANT_NAME):$(IMAGE_TAG)"
	docker build \
		$(DOCKER_BUILD_ARGS) \
		--build-arg BUILD_TYPE=prebuilt \
		--build-arg BUILD_IMAGE=$(BUILD_IMAGE) \
		--build-arg RUNTIME_IMAGE=$(RUNTIME_IMAGE) \
		--build-arg VENDOR_BASE_URL=$(VENDOR_BASE_URL) \
		--build-arg CIRCUITS_VERSION=$(CIRCUITS_VERSION) \
		-f Dockerfile.hierophant \
		-t $(HIEROPHANT_NAME):$(IMAGE_TAG) \
		.
	@echo "Build complete: $(HIEROPHANT_NAME):$(IMAGE_TAG)"
	@echo "  Build image:   $(BUILD_IMAGE)"
	@echo "  Runtime image: $(RUNTIME_IMAGE)"
	@echo "  Output tag:    $(CONTEMPLANT_NAME):$(IMAGE_TAG)"
	docker build \
		$(DOCKER_BUILD_ARGS) \
		--build-arg BUILD_TYPE=prebuilt \
		--build-arg BUILD_IMAGE=$(BUILD_IMAGE) \
		--build-arg RUNTIME_IMAGE=$(RUNTIME_IMAGE) \
		--build-arg VENDOR_BASE_URL=$(VENDOR_BASE_URL) \
		--build-arg CIRCUITS_VERSION=$(CIRCUITS_VERSION) \
		-f Dockerfile.contemplant \
		-t $(CONTEMPLANT_NAME):$(IMAGE_TAG) \
		.
	@echo "Build complete: $(CONTEMPLANT_NAME):$(IMAGE_TAG)"

.PHONY: run-h
run-h:
	@if [ ! -f .env ]; then \
		echo "ERROR: .env not found" >&2; \
		echo "Run 'make init' to create configuration files." >&2; \
		exit 1; \
	fi
	@if [ ! -f hierophant.toml ]; then \
		echo "ERROR: hierophant.toml not found" >&2; \
		echo "Run 'make init' to create configuration files." >&2; \
		exit 1; \
	fi
	@echo "Starting container ..."
	docker run --rm -it --init \
		--name $(HIEROPHANT_NAME) \
		$(DOCKER_RUN_ARGS) \
		--env-file .env \
		-v $(CURDIR)/hierophant.toml:/home/hierophant/hierophant.toml:ro \
		$(HIEROPHANT_NAME):$(IMAGE_TAG)

.PHONY: stop-h
stop-h:
	@echo "Stopping Hierophant container..."
	docker stop $(HIEROPHANT_NAME)
	docker rm $(HIEROPHANT_NAME) || true

.PHONY: run-c
run-c:
	@if [ ! -f .env ]; then \
		echo "ERROR: .env not found" >&2; \
		echo "Run 'make init' to create configuration files." >&2; \
		exit 1; \
	fi
	@if [ ! -f contemplant.toml ]; then \
		echo "ERROR: contemplant.toml not found" >&2; \
		echo "Run 'make init' to create configuration files." >&2; \
		exit 1; \
	fi
	@echo "Starting container ..."
	docker run -d --init \
		--gpus all \
		--name $(CONTEMPLANT_NAME) \
		$(DOCKER_RUN_ARGS) \
		--env-file .env \
		-v $(CURDIR)/contemplant.toml:/home/contemplant/contemplant.toml:ro \
		$(CONTEMPLANT_NAME):$(IMAGE_TAG)
	@echo "Waiting for services to start..."
	@sleep 5
	@echo "Attaching to tmux session (Ctrl+b, d to detach)..."
	docker exec -it $(CONTEMPLANT_NAME) tmux attach-session -t contemplant

.PHONY: stop-c
stop-c:
	@echo "Stopping Contemplant container..."
	docker stop $(CONTEMPLANT_NAME)
	docker rm $(CONTEMPLANT_NAME) || true

.PHONY: run
run:
	$(MAKE) build
	$(MAKE) ci
	@echo "Starting Hierophant and Contemplant ..."
	docker-compose -f docker-compose.run.yml up \
		--build \
		--abort-on-container-exit
	@echo "Cleaning up containers ..."
	docker-compose -f docker-compose.run.yml down -v
	@echo "... cleanup complete."

.PHONY: shell-h
shell-h:
	@echo "Opening shell in Hierophant ..."
	docker run --rm -it \
		--entrypoint /bin/bash \
		$(HIEROPHANT_NAME):$(IMAGE_TAG)

.PHONY: shell-c
shell-c:
	@echo "Opening shell in Contemplant ..."
	docker run --rm -it \
		--gpus all \
		--entrypoint /bin/bash \
		$(CONTEMPLANT_NAME):$(IMAGE_TAG)

.PHONY: act
act:
	@echo "Running GitHub Actions workflow locally with act ..."
	@if [ ! -d ".act-secrets" ]; then \
		echo "WARNING: .act-secrets/ directory not found" >&2; \
		echo "See docs/WORKFLOW_TESTING.md for setup instructions" >&2; \
	fi
	@echo "Cleaning previous act artifacts to prevent cross-repo contamination ..."
	@rm -rf /tmp/act-artifacts/*
	@echo "Setting up temporary secrets mount ..."
	@sudo mkdir -p /opt/github-runner
	@sudo rm -rf /opt/github-runner/secrets
	@sudo ln -s $(CURDIR)/.act-secrets /opt/github-runner/secrets
	@trap "sudo rm -f /opt/github-runner/secrets" EXIT; \
	DOCKER_HOST="" act push -W .github/workflows/release.yml \
		--container-options "-v /opt/github-runner/secrets:/opt/github-runner/secrets:ro" \
		--artifact-server-path=/tmp/act-artifacts \
		--pull=$(ACT_PULL) \
		$(if $(DOCKER_BUILD_ARGS),--env DOCKER_BUILD_ARGS="$(DOCKER_BUILD_ARGS)")

.PHONY: help
help:
	@echo "Build System"
	@echo ""
	@echo "Targets:"
	@echo "  init            Initialize config from examples."
	@echo "  circuits        Download and verify circuit artifacts."
	@echo "  moongate        Download and verify moongate-server binary."
	@echo "  clean           Clean output directories."
	@echo "  build           Build native binaries."
	@echo "  test            Run all tests for the build."
	@echo "  integration     Run end-to-end integration test with Docker Compose."
	@echo "  docker-h        Build just the Hierophant image."
	@echo "  docker-c        Build just the Contemplant image."
	@echo "  docker          Build Docker images (compiles inside container)."
	@echo "  ci              Build Docker images from pre-built binaries."
	@echo "  run-h           Run the built Hierophant locally."
	@echo "  run-c           Run the built Contemplant locally."
	@echo "  stop-h          Stop the running Hierophant."
	@echo "  stop-c          Stop the running Contemplant."
	@echo "  run             Run the built Docker images locally."
	@echo "  shell-h         Open a shell in the Hierophant image."
	@echo "  shell-c         Open a shell in the Contemplant image."
	@echo "  act             Test GitHub Actions release workflow locally."
	@echo "  help            Show this help message."
	@echo ""
	@echo "Configuration:"
	@echo "  Variables are loaded from .env.maintainer."
	@echo "  Override with environment variables:"
	@echo "    BUILD_IMAGE        - Builder image."
	@echo "    RUNTIME_IMAGE      - Runtime base image."
	@echo "    HIEROPHANT_NAME    - Hierophant Docker image name."
	@echo "    CONTEMPLANT_NAME   - Contemplant Docker image name."
	@echo "    IMAGE_TAG          - Docker image tag."
	@echo "    DOCKER_BUILD_ARGS  - Additional Docker build flags."
	@echo "    DOCKER_RUN_ARGS    - Additional Docker run flags."
	@echo ""
	@echo "Examples:"
	@echo "  make build"
	@echo "  BUILD_IMAGE=unattended/petros:latest make build"
	@echo "  IMAGE_TAG=v1.0.0 make build"
	@echo "  DOCKER_BUILD_ARGS='--network host' make build"
	@echo "  DOCKER_RUN_ARGS='--network host' make run-h"
	@echo "  DOCKER_RUN_ARGS='--network host' make run-c"

.DEFAULT_GOAL := build
