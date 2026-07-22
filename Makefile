CARGO := cargo
BIN_DIR ?= $(HOME)/bin
RUSH_BINS := amt board che macron

.PHONY: all build release install install-user fmt fmt-check lint test check clean help

all: check build

build: ## build all workspace binaries
	$(CARGO) build --workspace

release: ## build optimized workspace binaries
	$(CARGO) build --workspace --release --locked

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
