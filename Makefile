# lector — common tasks. Run `make help` for the list.

CARGO  ?= cargo
PYTHON ?= python3
PREFIX ?= $(HOME)/.local

VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
TARGET  := $(shell rustc -vV | sed -n 's/^host: //p')
NAME    := lector-$(VERSION)-$(TARGET)
DIST    := dist
SHA256  := $(shell command -v sha256sum 2>/dev/null || echo shasum -a 256)

.DEFAULT_GOAL := help
.PHONY: help build run test lint release install uninstall testdata clean

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

release: lint test ## Optimized build, packaged into dist/
	$(CARGO) build --release --locked
	rm -rf $(DIST)/$(NAME)
	mkdir -p $(DIST)/$(NAME)
	cp target/release/lector README.md $(DIST)/$(NAME)/
	cp target/release/lector $(DIST)/lector
	tar -czf $(DIST)/$(NAME).tar.gz -C $(DIST) $(NAME)
	cd $(DIST) && $(SHA256) $(NAME).tar.gz > $(NAME).tar.gz.sha256
	rm -rf $(DIST)/$(NAME)
	@echo
	@ls -lh $(DIST)/lector $(DIST)/$(NAME).tar.gz | awk '{print "  " $$5 "\t" $$9}'

install: release ## Install the release build to $(PREFIX)/bin
	mkdir -p $(PREFIX)/bin
	install -m 755 $(DIST)/lector $(PREFIX)/bin/lector

uninstall: ## Remove lector from $(PREFIX)/bin
	rm -f $(PREFIX)/bin/lector

testdata: ## Generate test documents in testdata/ (HUGE_MB=10 MANY=2000)
	$(PYTHON) scripts/gen_testdata.py testdata --huge-mb $(or $(HUGE_MB),10) --many $(or $(MANY),2000)

clean: ## Remove build output, dist/ and testdata/
	$(CARGO) clean
	rm -rf $(DIST) testdata
