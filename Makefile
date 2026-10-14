# Lege — common tasks. Run `make help` for the list.

CARGO  ?= cargo
PYTHON ?= python3
PREFIX ?= $(HOME)/.local
BIN    := lege

VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
# Rust target triple to build for; release CI sets it per platform.
TARGET  ?= $(shell rustc -vV | sed -n 's/^host: //p')
OS      := $(if $(findstring apple-darwin,$(TARGET)),darwin,linux)
ARCH    := $(if $(findstring aarch64,$(TARGET)),arm64,amd64)
# Same archive naming as the other yachiko projects: name_version_os_arch.
NAME    := lege_$(VERSION)_$(OS)_$(ARCH)
DIST    := dist
SHA256  := $(shell command -v sha256sum 2>/dev/null || echo shasum -a 256)

.DEFAULT_GOAL := help
.PHONY: help build run test lint release dist install uninstall testdata clean

help: ## Show this help
	@grep -E '^[a-z]+:.*## ' $(MAKEFILE_LIST) | awk -F ':.*## ' '{printf "  make %-10s %s\n", $$1, $$2}'

build: ## Debug build
	$(CARGO) build

run: ## Run a debug build on ARGS (default: .), e.g. make run ARGS=testdata/many
	$(CARGO) run -- $(or $(ARGS),.)

test: ## Run the tests
	$(CARGO) test

lint: ## Check formatting and run clippy
	$(CARGO) fmt --check
	$(CARGO) clippy --all-targets -- -D warnings

release: lint test dist ## Lint, test, then build and package for this machine

dist: ## Optimized build for TARGET, packaged as dist/lege_VERSION_OS_ARCH.tar.gz
	$(CARGO) build --release --locked --target $(TARGET)
	rm -rf $(DIST)/$(NAME)
	mkdir -p $(DIST)/$(NAME)
	cp target/$(TARGET)/release/$(BIN) LICENSE README.md $(DIST)/$(NAME)/
	cp target/$(TARGET)/release/$(BIN) $(DIST)/$(BIN)
	tar -czf $(DIST)/$(NAME).tar.gz -C $(DIST)/$(NAME) $(BIN) LICENSE README.md
	cd $(DIST) && $(SHA256) $(NAME).tar.gz > $(NAME).tar.gz.sha256
	rm -rf $(DIST)/$(NAME)
	@echo
	@ls -lh $(DIST)/$(BIN) $(DIST)/$(NAME).tar.gz | awk '{print "  " $$5 "\t" $$9}'

install: release ## Install lege to $(PREFIX)/bin
	mkdir -p $(PREFIX)/bin
	install -m 755 $(DIST)/$(BIN) $(PREFIX)/bin/$(BIN)

uninstall: ## Remove lege from $(PREFIX)/bin
	rm -f $(PREFIX)/bin/$(BIN)

testdata: ## Generate test documents in testdata/ (HUGE_MB=10 MANY=2000)
	$(PYTHON) scripts/gen_testdata.py testdata --huge-mb $(or $(HUGE_MB),10) --many $(or $(MANY),2000)

clean: ## Remove build output, dist/ and testdata/
	$(CARGO) clean
	rm -rf $(DIST) testdata
