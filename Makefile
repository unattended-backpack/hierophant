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
OPENVM_GIT_VERSION ?=
OPENVM_AGG_KEYS_VERSION ?=
OPENVM_EVM_ASSETS_VERSION ?=
OPENVM_KZG_VERSION ?= challenge_0085
DOCKER_BUILD_ARGS ?=
DOCKER_RUN_ARGS ?=

# DOCKER_BUILD_CACHE toggles the BuildKit cache mounts that give in-petros
# source builds warm incremental cargo state (see the Dockerfiles' build
# steps). 1 (default) reuses the shared cache namespace - one-line changes
# rebuild in minutes. 0 exercises the pristine no-local-state release flow:
# docker layer cache is bypassed (--no-cache) and the cargo cache mounts get
# a unique throwaway namespace, so the compile starts from nothing.
DOCKER_BUILD_CACHE ?= 1

# CUDA_ARCH overrides the GPU architecture list the CUDA kernel compiles
# target (openvm's CUDA_ARCH env + sppark/risc0's derived NVCC gencode
# flags). Unset uses the canonical release list from .env.maintainer
# (CUDA_RELEASE_ARCH; Ampere through Blackwell, Turing excluded because
# only risc0 fully supports it). Set a single arch (e.g. CUDA_ARCH=75)
# for ~5x faster kernel compiles during dev iteration; release builds
# must leave it unset.
CUDA_ARCH ?=
DOCKER_CUDA_ARCH := $(or $(CUDA_ARCH),$(CUDA_RELEASE_ARCH))

ifeq ($(DOCKER_BUILD_CACHE),0)
  BUILD_CACHE_FLAGS := --no-cache --build-arg BUILD_CACHE_ID=pristine-$(shell date +%s)
else
  BUILD_CACHE_FLAGS :=
endif
HIEROPHANT_NAME ?= hierophant
CONTEMPLANT_NAME ?= contemplant
IMAGE_TAG ?= latest
ACT_PULL ?= true

# BACKEND picks which proving backend the test targets exercise at runtime.
#   cpu           ; SP1 uses CpuProver; RISC Zero uses LocalProver.
#   cuda          ; SP1 spawns the vendored ~/.sp1/bin/sp1-gpu-server and
#                    talks to it over a unix socket (sp1-sdk 6.x replaced
#                    the moongate arrangement); RISC Zero uses in-process
#                    CUDA. Both require the container to be launched with
#                    GPU access, which the test compose files request via
#                    deploy.resources.reservations.devices.
# Defaults to cpu: every test target validates against cpu|cuda (the old
# `gpu` default matched neither and made bare `make test-*` invocations
# error out), and a cpu default keeps `make build` / `make run` working on
# hosts without a CUDA toolchain. Released binaries get their CUDA-enabled
# feature set from the release workflow independent of this default.
BACKEND ?= cpu

# Feature-set policy: docker (in-petros) builds default to the canonical
# release set from .env.maintainer - one universal binary for every VM and
# backend, so every test validates the release-shaped artifact. Native host
# builds default to the CPU-safe subset because host toolchains lack the
# pinned CUDA stack. An explicit CONTEMPLANT_FEATURES (command line or
# .env) overrides both.
CONTEMPLANT_RELEASE_FEATURES ?=
CONTEMPLANT_FEATURES ?=
DOCKER_CONTEMPLANT_FEATURES := $(or $(CONTEMPLANT_FEATURES),$(CONTEMPLANT_RELEASE_FEATURES))
NATIVE_CONTEMPLANT_FEATURES := $(or $(CONTEMPLANT_FEATURES),enable-native-gnark)

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

# The provision goals install the vendored prover assets a NATIVE
# (non-docker) contemplant needs onto this host, from the same CDN + sha256
# sidecars the image builds consume, so bare-metal workers run byte-identical
# assets to the containerized ones and no VM falls back to an unvendored
# upstream download at proof time. Docker deployments never need these; the
# images bake every asset in at build time.
#   provision         ; all three VMs.
#   provision-sp1     ; Groth16 + plonk circuits to ~/.sp1/circuits/ (the
#                        .download_complete markers stop sp1-sdk fetching
#                        from Succinct's S3 mid-proof) and sp1-gpu-server to
#                        the exact path sp1-cuda 6.x resolves (~/.sp1/bin/).
#   provision-risc0   ; the rzup-distributed risc0-groth16 component (the
#                        CUDA Groth16 path) to ~/.risc0/extensions/. The CPU
#                        Groth16 wrap on native hosts uses risc0's upstream
#                        docker-image path, which is NOT vendored; only the
#                        containerized contemplant replaces it with the
#                        vendored assets + docker shim.
#   provision-openvm  ; aggregation keys to ~/.openvm/. The ~13.5 GB EVM
#                        heavies (halo2.pk + KZG params) install only when
#                        OPENVM_EVM_ASSETS_VERSION is set, mirroring the
#                        image bake's gating; stark-only workers skip them.
.PHONY: provision
provision: provision-sp1 provision-risc0 provision-openvm
	@echo "All native prover assets provisioned."

.PHONY: provision-sp1
provision-sp1:
	@if [ -z "$(VENDOR_BASE_URL)" ] || [ -z "$(SP1_CIRCUITS_VERSION)" ]; then \
		echo "ERROR: VENDOR_BASE_URL and SP1_CIRCUITS_VERSION must be set" >&2; \
		echo "Load them from .env.maintainer or set as environment variables" >&2; \
		exit 1; \
	fi
	@echo "Provisioning SP1 assets (circuits $(SP1_CIRCUITS_VERSION)) ..."
	@mkdir -p ~/.sp1/circuits/groth16/$(SP1_CIRCUITS_VERSION)
	@mkdir -p ~/.sp1/circuits/plonk/$(SP1_CIRCUITS_VERSION)
	@mkdir -p ~/.sp1/bin
	@VENDOR_BASE_URL=$(VENDOR_BASE_URL) container/vendor.sh "groth16.tar.gz" "provers/sp1/$(SP1_CIRCUITS_VERSION)" "sp1/$(SP1_CIRCUITS_VERSION)/"
	@VENDOR_BASE_URL=$(VENDOR_BASE_URL) container/vendor.sh "plonk.tar.gz" "provers/sp1/$(SP1_CIRCUITS_VERSION)" "sp1/$(SP1_CIRCUITS_VERSION)/"
	@VENDOR_BASE_URL=$(VENDOR_BASE_URL) container/vendor.sh "sp1-gpu-server.tar.gz" "provers/sp1/$(SP1_CIRCUITS_VERSION)" "sp1/$(SP1_CIRCUITS_VERSION)/"
	@cp -r /tmp/extracted-groth16/$(SP1_CIRCUITS_VERSION)/* ~/.sp1/circuits/groth16/$(SP1_CIRCUITS_VERSION)/
	@cp -r /tmp/extracted-plonk/$(SP1_CIRCUITS_VERSION)/* ~/.sp1/circuits/plonk/$(SP1_CIRCUITS_VERSION)/
	@touch ~/.sp1/circuits/groth16/.download_complete
	@touch ~/.sp1/circuits/plonk/.download_complete
	@cp /tmp/extracted-sp1-gpu-server/sp1-gpu-server ~/.sp1/bin/sp1-gpu-server
	@chmod +x ~/.sp1/bin/sp1-gpu-server
	@rm -rf /tmp/extracted-groth16 /tmp/extracted-plonk /tmp/extracted-sp1-gpu-server
	@echo "SP1 assets provisioned."

.PHONY: provision-risc0
provision-risc0:
	@if [ -z "$(VENDOR_BASE_URL)" ] || [ -z "$(RISC0_GROTH16_RZUP_VERSION)" ]; then \
		echo "ERROR: VENDOR_BASE_URL and RISC0_GROTH16_RZUP_VERSION must be set" >&2; \
		echo "Load them from .env.maintainer or set as environment variables" >&2; \
		exit 1; \
	fi
	@echo "Provisioning RISC Zero assets (rzup component $(RISC0_GROTH16_RZUP_VERSION)) ..."
	@VENDOR_BASE_URL=$(VENDOR_BASE_URL) container/vendor.sh "risc0-groth16.tar.xz" "provers/risc0/groth16-rzup/$(RISC0_GROTH16_RZUP_VERSION)" "risc0/groth16-rzup/$(RISC0_GROTH16_RZUP_VERSION)/"
	@RZUP_VER="$(RISC0_GROTH16_RZUP_VERSION)"; RZUP_VER="$${RZUP_VER#v}"; \
	mkdir -p "$$HOME/.risc0/extensions/v$${RZUP_VER}-risc0-groth16" && \
	cp -r /tmp/extracted-risc0-groth16/* "$$HOME/.risc0/extensions/v$${RZUP_VER}-risc0-groth16/"
	@rm -rf /tmp/extracted-risc0-groth16
	@echo "RISC Zero assets provisioned."

.PHONY: provision-openvm
provision-openvm:
	@if [ -z "$(VENDOR_BASE_URL)" ] || [ -z "$(OPENVM_AGG_KEYS_VERSION)" ]; then \
		echo "ERROR: VENDOR_BASE_URL and OPENVM_AGG_KEYS_VERSION must be set" >&2; \
		echo "Load them from .env.maintainer or set as environment variables" >&2; \
		exit 1; \
	fi
	@echo "Provisioning OpenVM assets (aggregation keys $(OPENVM_AGG_KEYS_VERSION)) ..."
	@VENDOR_BASE_URL=$(VENDOR_BASE_URL) container/vendor.sh "openvm-agg-keys.tar.gz" "provers/openvm/$(OPENVM_AGG_KEYS_VERSION)" "openvm/$(OPENVM_AGG_KEYS_VERSION)/"
	@mkdir -p ~/.openvm
	@cp -r /tmp/extracted-openvm-agg-keys/* ~/.openvm/
	@rm -rf /tmp/extracted-openvm-agg-keys
	@if [ -n "$(OPENVM_EVM_ASSETS_VERSION)" ]; then \
		echo "Provisioning OpenVM EVM assets ($(OPENVM_EVM_ASSETS_VERSION); ~13.5 GB) ..."; \
		VENDOR_BASE_URL=$(VENDOR_BASE_URL) container/vendor.sh "openvm-halo2-pk.tar.gz" "provers/openvm/$(OPENVM_EVM_ASSETS_VERSION)" "openvm/$(OPENVM_EVM_ASSETS_VERSION)/" && \
		mkdir -p ~/.openvm/params && \
		mv /tmp/extracted-openvm-halo2-pk/halo2.pk ~/.openvm/halo2.pk && \
		rm -rf /tmp/extracted-openvm-halo2-pk && \
		for k in 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24; do \
			VENDOR_BASE_URL=$(VENDOR_BASE_URL) container/vendor.sh --file "kzg_bn254_$${k}.srs" "provers/openvm/kzg/$(OPENVM_KZG_VERSION)" "openvm/kzg/$(OPENVM_KZG_VERSION)/" && \
			mv "/tmp/kzg_bn254_$${k}.srs" ~/.openvm/params/ || exit 1; \
		done; \
	else \
		echo "OPENVM_EVM_ASSETS_VERSION empty; skipping EVM heavies (stark-only provisioning)."; \
	fi
	@echo "OpenVM assets provisioned."

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
	@echo "  Contemplant features: $(NATIVE_CONTEMPLANT_FEATURES)"
	mkdir -p out
	cargo build --release --bin hierophant
	cargo build --release --bin contemplant --features "$(NATIVE_CONTEMPLANT_FEATURES)"
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
#   cuda          ; SP1 CudaProver + the vendored in-image sp1-gpu-server
#                    (spawned by the contemplant itself over a unix socket).
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
# Every test target builds the same universal image pair from source inside
# petros (docker-h + docker-c) with the release feature set; BACKEND is
# purely a runtime selector, and every test validates the release-shaped
# artifact. BuildKit cache mounts (see DOCKER_BUILD_CACHE) make the repeat
# builds incremental, so this costs minutes, not the old full recompile.
# MODE has different defaults between test-sp1 (plonk) and test-risc0
# (composite). A single top-level `MODE ?= <default>` would bleed one default
# into the other target, so each recipe derives its own effective mode via
# shell parameter expansion on $${MODE:-<per-target-default>} instead.
.PHONY: test-sp1
test-sp1:
	@$(MAKE) docker-h
	@$(MAKE) docker-c
	@echo "Tearing down any lingering containers from a previous run ..."
	-docker-compose -f docker-compose.test.sp1.yml down -v
	@echo "Running SP1 integration test (BACKEND=$(BACKEND), MODE=$${MODE:-plonk}) ..."
	@echo "Starting Hierophant, Contemplant, and SP1 test client ..."
	@case "$(BACKEND)" in \
	  cpu)  CONTEMPLANT_SP1_BACKEND=cpu  ;; \
	  cuda) CONTEMPLANT_SP1_BACKEND=cuda ;; \
	  *) echo "unknown BACKEND=$(BACKEND); expected cpu|cuda" >&2; exit 1 ;; \
	esac; \
	MODE_EFF="$${MODE:-plonk}"; \
	case "$$MODE_EFF" in \
	  core|compressed|plonk|groth16) SP1_PROOF_SYSTEM=$$MODE_EFF ;; \
	  *) echo "unknown MODE=$$MODE_EFF; expected core|compressed|plonk|groth16" >&2; exit 1 ;; \
	esac; \
	export CONTEMPLANT_SP1_BACKEND SP1_PROOF_SYSTEM \
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
	@$(MAKE) docker-h
	@$(MAKE) docker-c
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

# MODE selects which OpenVM proving path the integration test exercises:
#   app (default); app-level continuation STARK, cheapest path.
#   stark        ; aggregated root STARK; single compact proof. The worker
#                   runs aggregation keygen in-process unless ~/.openvm/
#                   artifacts from `cargo openvm setup` are staged in the
#                   image; expect a long, RAM-hungry first proof.
#   evm          ; halo2-wrapped EVM-verifiable proof. Requires the
#                   contemplant to be built with enable-openvm-evm (this
#                   target derives that automatically) AND the KZG params +
#                   halo2 keys of `cargo openvm setup --evm` (~70 GB RAM to
#                   generate in-process otherwise); expect this to be viable
#                   only on very large machines.
# Usage: `make test-openvm`                 # app, cpu
#        `make test-openvm MODE=stark`      # stark, cpu
#        `make test-openvm BACKEND=cuda`    # app, gpu (needs enable-openvm-cuda build)
.PHONY: test-openvm
test-openvm:
	@$(MAKE) docker-h
	@$(MAKE) docker-c
	@echo "Tearing down any lingering containers from a previous run ..."
	-docker-compose -f docker-compose.test.openvm.yml down -v
	@echo "Running OpenVM integration test (MODE=$${MODE:-app}, BACKEND=$(BACKEND)) ..."
	@echo "Starting Hierophant, Contemplant, and OpenVM test client ..."
	@case "$(BACKEND)" in cpu|cuda) ;; *) echo "unknown BACKEND=$(BACKEND); expected cpu|cuda" >&2; exit 1 ;; esac
	@MODE_EFF="$${MODE:-app}"; \
	case "$$MODE_EFF" in \
	  app)   PROOF_MODE=app   CONTEMPLANT_OPENVM_EVM=false ;; \
	  stark) PROOF_MODE=stark CONTEMPLANT_OPENVM_EVM=false ;; \
	  evm)   PROOF_MODE=evm   CONTEMPLANT_OPENVM_EVM=true  ;; \
	  *) echo "unknown MODE=$$MODE_EFF; expected app|stark|evm" >&2; exit 1 ;; \
	esac; \
	export PROOF_MODE CONTEMPLANT_OPENVM_EVM CONTEMPLANT_OPENVM_BACKEND=$(BACKEND) \
	  VENDOR_BASE_URL="$(VENDOR_BASE_URL)" OPENVM_GIT_VERSION="$(OPENVM_GIT_VERSION)"; \
	docker-compose -f docker-compose.test.openvm.yml up \
		--build \
		--force-recreate \
		--abort-on-container-exit \
		--exit-code-from test-client
	@echo "Cleaning up containers ..."
	-docker-compose -f docker-compose.test.openvm.yml down -v
	@echo "OpenVM integration test complete (MODE=$${MODE:-app}, BACKEND=$(BACKEND))."

# Runs the full capability matrix: every MODE/BACKEND combination of all
# three zkVM test targets, against one universal image pair built up front.
# A failing combination does not stop the run; every combination executes
# and the summary at the end reports PASS or FAIL for each, with the target
# exiting nonzero if anything failed. Each combination is capped at
# TEST_ALL_TIMEOUT seconds (45 minutes by default) so a hung proof cannot
# stall the matrix; a timeout counts as FAIL, and the combination's compose
# stack is torn down before the next one starts. Expect hardware-dependent
# results per combination (SP1 CUDA requires a 24 GB GPU; see README).
# Usage: `make test-all`
#        `make test-all TEST_ALL_TIMEOUT=7200`   # slower hardware
TEST_ALL_TIMEOUT ?= 2700
.PHONY: test-all
test-all:
	@$(MAKE) docker
	@overall=0; summary=""; \
	for combo in \
	  sp1:core:cpu sp1:compressed:cpu sp1:plonk:cpu sp1:groth16:cpu \
	  sp1:core:cuda sp1:compressed:cuda sp1:plonk:cuda sp1:groth16:cuda \
	  risc0:composite:cpu risc0:succinct:cpu risc0:groth16:cpu risc0:groth16-direct:cpu \
	  risc0:composite:cuda risc0:succinct:cuda risc0:groth16:cuda risc0:groth16-direct:cuda \
	  openvm:app:cpu openvm:stark:cpu openvm:evm:cpu \
	  openvm:app:cuda openvm:stark:cuda openvm:evm:cuda; \
	do \
	  vm="$${combo%%:*}"; rest="$${combo#*:}"; \
	  mode="$${rest%%:*}"; backend="$${rest#*:}"; \
	  echo ""; echo "=== test-all: $$vm MODE=$$mode BACKEND=$$backend ==="; \
	  if timeout --foreground -k 60 $(TEST_ALL_TIMEOUT) \
	    $(MAKE) "test-$$vm" MODE="$$mode" BACKEND="$$backend"; then \
	    status=PASS; \
	  else \
	    status=FAIL; overall=1; \
	  fi; \
	  docker-compose -f "docker-compose.test.$$vm.yml" down -v >/dev/null 2>&1 || true; \
	  summary="$$summary$$(printf '%-8s %-16s %-8s %s' "$$vm" "$$mode" "$$backend" "$$status")\n"; \
	done; \
	echo ""; echo "=== test-all summary ==="; \
	printf '%-8s %-16s %-8s %s\n' VM MODE BACKEND STATUS; \
	printf '%b' "$$summary"; \
	exit $$overall

.PHONY: docker-h
docker-h:
	@echo "Building Hierophant image ..."
	@echo "  Build image:   $(BUILD_IMAGE)"
	@echo "  Runtime image: $(RUNTIME_IMAGE)"
	@echo "  Output tag:    $(HIEROPHANT_NAME):$(IMAGE_TAG)"
	@mkdir -p out
	docker build \
		$(DOCKER_BUILD_ARGS) \
		$(BUILD_CACHE_FLAGS) \
		--build-arg BUILD_IMAGE=$(BUILD_IMAGE) \
		--build-arg RUNTIME_IMAGE=$(RUNTIME_IMAGE) \
		--build-arg VENDOR_BASE_URL=$(VENDOR_BASE_URL) \
		--build-arg SP1_CIRCUITS_VERSION=$(SP1_CIRCUITS_VERSION) \
		--build-arg OPENVM_GIT_VERSION=$(OPENVM_GIT_VERSION) \
		--build-arg OPENVM_AGG_KEYS_VERSION=$(OPENVM_AGG_KEYS_VERSION) \
		-f Dockerfile.hierophant \
		-t $(HIEROPHANT_NAME):$(IMAGE_TAG) \
		.
	@echo "Build complete: $(HIEROPHANT_NAME):$(IMAGE_TAG)"

.PHONY: docker-c
docker-c:
	@echo "Building Contemplant image ..."
	@echo "  Build image:   $(BUILD_IMAGE)"
	@echo "  Runtime image: $(RUNTIME_IMAGE)"
	@echo "  Features:      $(DOCKER_CONTEMPLANT_FEATURES)"
	@echo "  Output tag:    $(CONTEMPLANT_NAME):$(IMAGE_TAG)"
	@mkdir -p out
	docker build \
		$(DOCKER_BUILD_ARGS) \
		$(BUILD_CACHE_FLAGS) \
		$(if $(DOCKER_CUDA_ARCH),--build-arg CUDA_ARCH=$(DOCKER_CUDA_ARCH)) \
		--build-arg BUILD_IMAGE=$(BUILD_IMAGE) \
		--build-arg RUNTIME_IMAGE=$(RUNTIME_IMAGE) \
		--build-arg VENDOR_BASE_URL=$(VENDOR_BASE_URL) \
		--build-arg SP1_CIRCUITS_VERSION=$(SP1_CIRCUITS_VERSION) \
		--build-arg RISC0_GROTH16_PROVER_TAG=$(RISC0_GROTH16_PROVER_TAG) \
		--build-arg RISC0_GROTH16_RZUP_VERSION=$(RISC0_GROTH16_RZUP_VERSION) \
		--build-arg OPENVM_GIT_VERSION=$(OPENVM_GIT_VERSION) \
		--build-arg OPENVM_AGG_KEYS_VERSION=$(OPENVM_AGG_KEYS_VERSION) \
		--build-arg OPENVM_EVM_ASSETS_VERSION=$(OPENVM_EVM_ASSETS_VERSION) \
		--build-arg OPENVM_KZG_VERSION=$(OPENVM_KZG_VERSION) \
		--build-arg CONTEMPLANT_FEATURES=$(DOCKER_CONTEMPLANT_FEATURES) \
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
		--build-arg OPENVM_GIT_VERSION=$(OPENVM_GIT_VERSION) \
		--build-arg OPENVM_AGG_KEYS_VERSION=$(OPENVM_AGG_KEYS_VERSION) \
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
		--build-arg OPENVM_GIT_VERSION=$(OPENVM_GIT_VERSION) \
		--build-arg OPENVM_AGG_KEYS_VERSION=$(OPENVM_AGG_KEYS_VERSION) \
		--build-arg OPENVM_EVM_ASSETS_VERSION=$(OPENVM_EVM_ASSETS_VERSION) \
		--build-arg OPENVM_KZG_VERSION=$(OPENVM_KZG_VERSION) \
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
	@echo "  sp1-gpu-server  Download and install the SP1 CUDA prover server."
	@echo "  clean           Clean output directories."
	@echo "  build           Build native binaries."
	@echo "  test            Run all tests for the build."
	@echo "  test-sp1        Run end-to-end SP1 integration test with Docker Compose."
	@echo "  test-risc0      Run end-to-end RISC Zero integration test with Docker Compose."
	@echo "  test-openvm     Run end-to-end OpenVM integration test with Docker Compose."
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
