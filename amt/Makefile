BINARY_NAME := amt
VERSION := $(shell cat VERSION)
CARGO := cargo
BIN_DIR ?= $(HOME)/bin

.PHONY: all build compile release install install-bin install-cargo uninstall run test fmt fmt-check lint check pre clean version help

all: check build

build: ## build the binary
	$(CARGO) build

compile: build ## alias for build

release: ## build the optimized release binary
	$(CARGO) build --release --locked

install: release ## install into BIN_DIR, default ~/bin
	install -d "$(BIN_DIR)"
	install -m 0755 "target/release/$(BINARY_NAME)" "$(BIN_DIR)/$(BINARY_NAME)"

install-bin: install ## alias for install

install-cargo: ## install with cargo install --path .
	$(CARGO) install --path . --locked

uninstall: ## uninstall from Cargo's local bin directory
	$(CARGO) uninstall $(BINARY_NAME)

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

clean: ## remove Cargo build artifacts
	$(CARGO) clean

version: ## print project version
	@printf "VERSION=%s\n" "$(VERSION)"
	@$(CARGO) run --quiet -- version

help: ## show help message
	@awk 'BEGIN {FS = ":.*##"; printf "\nUsage:\n"} /^[a-zA-Z0-9_-]+:.*?##/ { printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2 }' $(MAKEFILE_LIST)
