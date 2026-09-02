CARGO := cargo
BIN_DIR ?= $(HOME)/bin
DIST_DIR ?= dist
RUSH_BINS := amt board che macron mux textwarrior

.PHONY: all build release release-macos release-macos-arm64 release-macos-amd64 release-target install install-user fmt fmt-check lint test check clean help

all: check build

build: ## build all workspace binaries
	$(CARGO) build --workspace

release: ## build optimized workspace binaries
	$(CARGO) build --workspace --release --locked

release-macos: release-macos-arm64 release-macos-amd64 ## build macOS ARM64 and AMD64 release bundles

release-macos-arm64: ## build a macOS ARM64 release bundle
	$(MAKE) release-target TARGET=aarch64-apple-darwin

release-macos-amd64: ## build a macOS AMD64 release bundle
	$(MAKE) release-target TARGET=x86_64-apple-darwin

release-target: ## build all binaries for TARGET into DIST_DIR/TARGET
	rustup target add "$(TARGET)"
	$(CARGO) build --workspace --release --locked --target "$(TARGET)"
	install -d "$(DIST_DIR)/$(TARGET)"
	for bin in $(RUSH_BINS); do install -m 0755 "target/$(TARGET)/release/$$bin" "$(DIST_DIR)/$(TARGET)/$$bin"; done

install: release ## install rush binaries into BIN_DIR, default ~/bin
	install -d "$(BIN_DIR)"
	for bin in $(RUSH_BINS); do install -m 0755 "target/release/$$bin" "$(BIN_DIR)/$$bin"; done

install-user: install ## alias for install

fmt: ## format Rust code
	$(CARGO) fmt

fmt-check: ## check Rust formatting
	$(CARGO) fmt -- --check

lint: ## run clippy with warnings as errors
	$(CARGO) clippy --workspace --all-targets --all-features -- -D warnings

test: ## run workspace tests
	$(CARGO) test --workspace

check: fmt-check lint test ## run formatting, linting, and tests

clean: ## remove Cargo build artifacts
	$(CARGO) clean

help: ## show help message
	@awk 'BEGIN {FS = ":.*##"; printf "\nUsage:\n"} /^[a-zA-Z0-9_-]+:.*?##/ { printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2 }' $(MAKEFILE_LIST)
