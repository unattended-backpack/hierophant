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
SP1_CIRCUITS_VERSION ?=
RISC0_GROTH16_PROVER_TAG ?=
RISC0_GROTH16_RZUP_VERSION ?=
DOCKER_BUILD_ARGS ?=
DOCKER_RUN_ARGS ?=
HIEROPHANT_NAME ?= hierophant
CONTEMPLANT_NAME ?= contemplant
IMAGE_TAG ?= latest
ACT_PULL ?= true

# BACKEND picks which proving backend the test targets exercise at runtime.
#   cpu           ; SP1 uses CpuProver; RISC Zero uses LocalProver.
#   cuda          ; SP1 talks to an in-container moongate-server on :3000
#                    (entrypoint auto-starts it via tmux); RISC Zero uses
#                    in-process CUDA. Both require the container to be
#                    launched with GPU access, which the test compose files
#                    request via deploy.resources.reservations.devices.
BACKEND ?= gpu

# CONTEMPLANT_FEATURES is derived from BACKEND by default so a CPU build
# doesn't pay the (expensive, nvcc-version-sensitive) cost of compiling
# risc0's CUDA kernels. An explicit override still wins; set
# `CONTEMPLANT_FEATURES=...` on the command line or in .env to force a
# specific feature set regardless of BACKEND.
ifndef CONTEMPLANT_FEATURES
  ifeq ($(BACKEND),cuda)
    CONTEMPLANT_FEATURES := enable-native-gnark,enable-risc0-cuda
  else
    CONTEMPLANT_FEATURES := enable-native-gnark
  endif
endif

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
	@if [ -z "$(VENDOR_BASE_URL)" ] || [ -z "$(SP1_CIRCUITS_VERSION)" ]; then \
		echo "ERROR: VENDOR_BASE_URL and SP1_CIRCUITS_VERSION must be set" >&2; \
		echo "Load them from .env.maintainer or set as environment variables" >&2; \
		exit 1; \
	fi
	@echo "  Vendor URL: $(VENDOR_BASE_URL)"
	@echo "  Circuits version: $(SP1_CIRCUITS_VERSION)"
	@mkdir -p ~/.sp1/circuits/groth16/$(SP1_CIRCUITS_VERSION)
	@mkdir -p ~/.sp1/circuits/plonk/$(SP1_CIRCUITS_VERSION)
	@echo "Downloading Groth16 circuits ..."
	@VENDOR_BASE_URL=$(VENDOR_BASE_URL) container/vendor.sh "groth16.tar.gz" "provers/sp1/$(SP1_CIRCUITS_VERSION)" "sp1/$(SP1_CIRCUITS_VERSION)/"
	@echo "Downloading plonk circuits ..."
	@VENDOR_BASE_URL=$(VENDOR_BASE_URL) container/vendor.sh "plonk.tar.gz" "provers/sp1/$(SP1_CIRCUITS_VERSION)" "sp1/$(SP1_CIRCUITS_VERSION)/"
	@echo "Installing to ~/.sp1/circuits/ ..."
	@cp -r /tmp/extracted-groth16/$(SP1_CIRCUITS_VERSION)/* ~/.sp1/circuits/groth16/$(SP1_CIRCUITS_VERSION)/
	@cp -r /tmp/extracted-plonk/$(SP1_CIRCUITS_VERSION)/* ~/.sp1/circuits/plonk/$(SP1_CIRCUITS_VERSION)/
	@touch ~/.sp1/circuits/groth16/.download_complete
	@touch ~/.sp1/circuits/plonk/.download_complete
	@rm -rf /tmp/extracted-groth16 /tmp/extracted-plonk
	@echo "Circuit artifacts installed successfully."

.PHONY: moongate
moongate:
	@echo "Downloading and verifying moongate-server ..."
	@if [ -z "$(VENDOR_BASE_URL)" ] || [ -z "$(SP1_CIRCUITS_VERSION)" ]; then \
		echo "ERROR: VENDOR_BASE_URL and SP1_CIRCUITS_VERSION must be set" >&2; \
		echo "Load them from .env.maintainer or set as environment variables" >&2; \
		exit 1; \
	fi
	@echo "  Vendor URL: $(VENDOR_BASE_URL)"
	@echo "  SP1 version: $(SP1_CIRCUITS_VERSION)"
	@echo "Downloading moongate-server ..."
	@VENDOR_BASE_URL=$(VENDOR_BASE_URL) container/vendor.sh "moongate-server.tar.gz" "provers/sp1/$(SP1_CIRCUITS_VERSION)" "sp1/$(SP1_CIRCUITS_VERSION)/"
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
	@echo "  Contemplant features: $(CONTEMPLANT_FEATURES)"
	mkdir -p out
	cargo build --release --bin hierophant
	cargo build --release --bin contemplant --features "$(CONTEMPLANT_FEATURES)"
	cp ./target/release/hierophant ./out/hierophant
	cp ./target/release/contemplant ./out/contemplant
	@echo "Build complete."

.PHONY: test
test:
	@echo "Running tests ..."
	cargo test --release
	@echo "... tests completed."

# BACKEND selects the runtime proving backend for the SP1 contemplant:
#   cpu  (default); SP1 CpuProver, no GPU needed.
#   cuda          ; SP1 CudaProver + in-container moongate-server (tmux'd
#                    by the entrypoint when MOONGATE_ENDPOINT is set).
#                    Requires a GPU-capable host; the compose file reserves
#                    the GPU via deploy.resources.reservations.devices.
# MODE selects which SP1 proving mode the integration test exercises:
#   core      ; raw STARK, cheapest path, not EVM-verifiable.
#   compressed; core compressed into a single recursive STARK.
#   plonk     ; EVM-verifiable Plonk SNARK wrap.
#   groth16   ; EVM-verifiable Groth16 SNARK wrap (smaller proof, heavier prove).
# Usage: `make test-sp1`                     # plonk, cpu
#        `make test-sp1 MODE=core`           # core, cpu
#        `make test-sp1 BACKEND=cuda`        # plonk, gpu
#        `make test-sp1 MODE=groth16 BACKEND=cuda`
#
# Build path depends on BACKEND:
#   cpu ; fast path: native cargo build on host → prebuilt docker layer.
#   cuda; host cargo can't build the CUDA kernels (your host's nvcc /
#          sppark combo may not be compatible; petros has the pinned
#          CUDA 12.9 toolchain). Build from source inside petros via
#          `docker-c` instead.
# MODE has different defaults between test-sp1 (plonk) and test-risc0
# (composite). A single top-level `MODE ?= <default>` would bleed one default
# into the other target, so each recipe derives its own effective mode via
# shell parameter expansion on $${MODE:-<per-target-default>} instead.
.PHONY: test-sp1
test-sp1:
	@if [ "$(BACKEND)" = "cuda" ]; then \
		$(MAKE) docker-h ; \
		$(MAKE) docker-c ; \
	else \
		$(MAKE) build ; \
		$(MAKE) ci ; \
	fi
	@echo "Tearing down any lingering containers from a previous run ..."
	-docker-compose -f docker-compose.test.sp1.yml down -v
	@echo "Running SP1 integration test (BACKEND=$(BACKEND), MODE=$${MODE:-plonk}) ..."
	@echo "Starting Hierophant, Contemplant, and SP1 test client ..."
	@case "$(BACKEND)" in \
	  cpu)  CONTEMPLANT_SP1_BACKEND=cpu  MOONGATE_ENDPOINT= ;; \
	  cuda) CONTEMPLANT_SP1_BACKEND=cuda MOONGATE_ENDPOINT=http://localhost:3000/twirp/ ;; \
	  *) echo "unknown BACKEND=$(BACKEND); expected cpu|cuda" >&2; exit 1 ;; \
	esac; \
	MODE_EFF="$${MODE:-plonk}"; \
	case "$$MODE_EFF" in \
	  core|compressed|plonk|groth16) SP1_PROOF_SYSTEM=$$MODE_EFF ;; \
	  *) echo "unknown MODE=$$MODE_EFF; expected core|compressed|plonk|groth16" >&2; exit 1 ;; \
	esac; \
	export CONTEMPLANT_SP1_BACKEND MOONGATE_ENDPOINT SP1_PROOF_SYSTEM \
	  VENDOR_BASE_URL="$(VENDOR_BASE_URL)" SP1_CIRCUITS_VERSION="$(SP1_CIRCUITS_VERSION)"; \
	docker-compose -f docker-compose.test.sp1.yml up \
		--build \
		--force-recreate \
		--abort-on-container-exit \
		--exit-code-from test-client
	@echo "Cleaning up containers ..."
	-docker-compose -f docker-compose.test.sp1.yml down -v
	@echo "SP1 integration test complete (BACKEND=$(BACKEND), MODE=$${MODE:-plonk})."

# MODE selects which RISC Zero proving path the integration test exercises:
#   composite (default); STARK, cheapest path.
#   succinct           ; recursed STARK; single segment; bigger-than-composite.
#   groth16            ; STARK session + /snark/create wrap into an onchain
#                         Groth16 seal. Requires the groth16-enabled contemplant
#                         path (vendored assets + docker shim from
#                         Dockerfile.contemplant).
#   groth16-direct     ; PROOF_MODE=groth16 without the wrap step (skips the
#                         typical Bonsai two-step flow; rarely used, exposed
#                         for completeness).
# Usage: `make test-risc0`                        # composite, cpu
#        `make test-risc0 MODE=succinct`          # succinct, cpu
#        `make test-risc0 MODE=groth16`           # wrap-to-groth16, cpu
#        `make test-risc0 BACKEND=cuda`           # composite, gpu
#        `make test-risc0 MODE=groth16 BACKEND=cuda`
.PHONY: test-risc0
test-risc0:
	@if [ "$(BACKEND)" = "cuda" ]; then \
		$(MAKE) docker-h ; \
		$(MAKE) docker-c ; \
	else \
		$(MAKE) build ; \
		$(MAKE) ci ; \
	fi
	@echo "Tearing down any lingering containers from a previous run ..."
	-docker-compose -f docker-compose.test.risc0.yml down -v
	@echo "Running RISC Zero integration test (MODE=$${MODE:-composite}, BACKEND=$(BACKEND)) ..."
	@echo "Starting Hierophant, Contemplant, and RISC Zero test client ..."
	@case "$(BACKEND)" in cpu|cuda) ;; *) echo "unknown BACKEND=$(BACKEND); expected cpu|cuda" >&2; exit 1 ;; esac
	@MODE_EFF="$${MODE:-composite}"; \
	case "$$MODE_EFF" in \
	  composite)      PROOF_MODE=composite WRAP_SNARK=false CONTEMPLANT_RISC0_GROTH16=false ;; \
	  succinct)       PROOF_MODE=succinct  WRAP_SNARK=false CONTEMPLANT_RISC0_GROTH16=false ;; \
	  groth16)        PROOF_MODE=composite WRAP_SNARK=true  CONTEMPLANT_RISC0_GROTH16=true  ;; \
	  groth16-direct) PROOF_MODE=groth16   WRAP_SNARK=false CONTEMPLANT_RISC0_GROTH16=true  ;; \
	  *) echo "unknown MODE=$$MODE_EFF; expected composite|succinct|groth16|groth16-direct" >&2; exit 1 ;; \
	esac; \
	export PROOF_MODE WRAP_SNARK CONTEMPLANT_RISC0_GROTH16 CONTEMPLANT_RISC0_BACKEND=$(BACKEND); \
	docker-compose -f docker-compose.test.risc0.yml up \
		--build \
		--force-recreate \
		--abort-on-container-exit \
		--exit-code-from test-client
	@echo "Cleaning up containers ..."
	-docker-compose -f docker-compose.test.risc0.yml down -v
	@echo "RISC Zero integration test complete (MODE=$${MODE:-composite}, BACKEND=$(BACKEND))."

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
		--build-arg SP1_CIRCUITS_VERSION=$(SP1_CIRCUITS_VERSION) \
		-f Dockerfile.hierophant \
		-t $(HIEROPHANT_NAME):$(IMAGE_TAG) \
		.
	@echo "Build complete: $(HIEROPHANT_NAME):$(IMAGE_TAG)"

.PHONY: docker-c
docker-c:
	@echo "Building Contemplant image ..."
	@echo "  Build image:   $(BUILD_IMAGE)"
	@echo "  Runtime image: $(RUNTIME_IMAGE)"
	@echo "  Features:      $(CONTEMPLANT_FEATURES)"
	@echo "  Output tag:    $(CONTEMPLANT_NAME):$(IMAGE_TAG)"
	@mkdir -p out
	docker build \
		$(DOCKER_BUILD_ARGS) \
		--build-arg BUILD_IMAGE=$(BUILD_IMAGE) \
		--build-arg RUNTIME_IMAGE=$(RUNTIME_IMAGE) \
		--build-arg VENDOR_BASE_URL=$(VENDOR_BASE_URL) \
		--build-arg SP1_CIRCUITS_VERSION=$(SP1_CIRCUITS_VERSION) \
		--build-arg RISC0_GROTH16_PROVER_TAG=$(RISC0_GROTH16_PROVER_TAG) \
		--build-arg RISC0_GROTH16_RZUP_VERSION=$(RISC0_GROTH16_RZUP_VERSION) \
		--build-arg CONTEMPLANT_FEATURES=$(CONTEMPLANT_FEATURES) \
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
		--build-arg SP1_CIRCUITS_VERSION=$(SP1_CIRCUITS_VERSION) \
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
		--build-arg SP1_CIRCUITS_VERSION=$(SP1_CIRCUITS_VERSION) \
		--build-arg RISC0_GROTH16_PROVER_TAG=$(RISC0_GROTH16_PROVER_TAG) \
		--build-arg RISC0_GROTH16_RZUP_VERSION=$(RISC0_GROTH16_RZUP_VERSION) \
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
	@echo "  test-sp1        Run end-to-end SP1 integration test with Docker Compose."
	@echo "  test-risc0      Run end-to-end RISC Zero integration test with Docker Compose."
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
