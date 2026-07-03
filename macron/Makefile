BINARY_NAME := macron
VERSION := 0.1.0
CARGO := cargo
UNAME_S := $(shell uname -s)
UNAME_M := $(shell uname -m)
MACOS_TARGET ?= aarch64-apple-darwin
MACOS_ARCH := $(if $(findstring aarch64,$(MACOS_TARGET)),arm64,x86_64)
CARGO_TARGET_DIR ?= ../target
export CARGO_TARGET_DIR

.PHONY: all build release install uninstall macos-binary macos-cross-binary run test fmt fmt-check lint check pre install-hooks clean help

all: check build

build: ## build the binary
	$(CARGO) build

release: ## build the optimized release binary
	$(CARGO) build --release --locked

install: ## install macron locally
	$(CARGO) install --path . --locked

uninstall: ## uninstall macron from Cargo's local bin directory
	$(CARGO) uninstall $(BINARY_NAME)

macos-binary: ## build an optimized native macOS binary in dist/
	@test "$(UNAME_S)" = "Darwin" || (echo "macos-binary must be run on macOS; for Linux cross-builds use: make macos-cross-binary MACOS_TARGET=aarch64-apple-darwin" && exit 1)
	$(CARGO) build --release --locked
	mkdir -p dist
	cp $(CARGO_TARGET_DIR)/release/$(BINARY_NAME) dist/$(BINARY_NAME)-v$(VERSION)-macos-$(UNAME_M)

macos-cross-binary: ## cross-build a macOS binary using a configured Darwin SDK/linker toolchain
	rustup target add $(MACOS_TARGET)
	$(CARGO) build --release --locked --target $(MACOS_TARGET)
	mkdir -p dist
	cp $(CARGO_TARGET_DIR)/$(MACOS_TARGET)/release/$(BINARY_NAME) dist/$(BINARY_NAME)-v$(VERSION)-macos-$(MACOS_ARCH)

run: ## run the binary, pass args with ARGS="..."
	$(CARGO) run -- $(ARGS)

test: ## run tests
	$(CARGO) test

fmt: ## format Rust code
	$(CARGO) fmt

fmt-check: ## check Rust formatting
	$(CARGO) fmt -- --check

lint: ## run clippy with warnings as errors
	$(CARGO) clippy --all-targets --all-features -- -D warnings

check: fmt-check lint test ## run formatting, linting, and tests

pre: ## run pre-commit on all files
	pre-commit run --all-files

install-hooks: ## install pre-commit hooks
	pre-commit install

clean: ## remove Cargo build artifacts
	$(CARGO) clean

help: ## show help message
	@awk 'BEGIN {FS = ":.*##"; printf "\nUsage:\n"} /^[a-zA-Z0-9_-]+:.*?##/ { printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2 }' $(MAKEFILE_LIST)
