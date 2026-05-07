# Makefile for grug-brain
# Common development targets.

.PHONY: build test test-rust test-playwright install-playwright

## Build the release binary.
build:
	cargo build --release

## Run all Rust tests (unit + integration).
test-rust:
	cargo test

## Install Playwright browser binaries (run once after `npm install`).
install-playwright:
	cd tests/playwright && npm install && npx playwright install chromium

## Run Playwright end-to-end tests for the web viewer.
## Requires: cargo build (debug binary at target/debug/grug) must be up to date.
## First run: `make install-playwright` to install browser binaries.
##
## Usage:
##   make test-playwright            # run all Playwright tests
##   make test-playwright GREP=4.7   # run one test by pattern
test-playwright:
	@if [ ! -f target/debug/grug ]; then \
		echo "Debug binary not found — running cargo build..."; \
		cargo build; \
	fi
	cd tests/playwright && npx playwright test $(if $(GREP),--grep "$(GREP)",)

## Run both Rust and Playwright test suites.
test: test-rust test-playwright
