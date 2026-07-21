# Developer entrypoints for lightstreamer-rs.
#
# Every target here is what `.github/workflows/ci.yml` runs -- `make pre-push`
# is intentionally the same set of targets CI runs, so "green locally" means
# CI will be green too. Targets are idempotent and safe to re-run.
#
# `default = []` in Cargo.toml, so `cargo test` and `cargo test
# --no-default-features` are equivalent today; both are kept as separate
# targets because `json-patch` and `test-util` change the compiled surface
# and a future default-feature change must not silently drop a leg.

SHELL := /bin/bash
.PHONY: help all build release test test-default test-all-features \
        test-no-default-features fmt fmt-check lint lint-fix doc deny \
        msrv package-check check-spanish pre-push check clean release-check

# The declared MSRV, read from Cargo.toml so `make msrv` and the CI MSRV job
# always exercise whatever version is currently declared there.
MSRV := $(shell grep -m1 '^rust-version' Cargo.toml | sed -E 's/.*"([^"]*)".*/\1/')

# Character class the Spanish-comment check looks for.
CHECK_SPANISH_CHARS := áéíóúñÁÉÍÓÚÑ¿¡
# Files intentionally excluded from the Spanish-comment check: they use
# accented Latin characters (`é`, `ñ`) as literal UTF-8 test/example data for
# TLCP percent-decoding and character-diff fixtures, not as Spanish-language
# comments. Every other tracked `.rs` file under src/ and examples/ is
# checked, including any new file added later. Computed once here (not
# inline in the recipe) so the recipe needs no nested shell quoting.
CHECK_SPANISH_EXCLUDE := src/protocol/escaping.rs src/subscription/update.rs
CHECK_SPANISH_EXCLUDE_RE := $(shell echo $(CHECK_SPANISH_EXCLUDE) | tr ' ' '|')

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-24s\033[0m %s\n", $$1, $$2}'

all: test fmt lint build ## Run the full local test+format+lint+build pass

build: ## Debug build with all features
	cargo build --all-features

release: ## Release build with all features (zero warnings required)
	cargo build --release --all-features

test-default: ## cargo test, default features
	cargo test

test-all-features: ## cargo test --all-features
	cargo test --all-features

test-no-default-features: ## cargo test --no-default-features
	cargo test --no-default-features

test: test-default test-all-features test-no-default-features ## Run the full feature-matrix test suite

fmt: ## Format the tree in place
	cargo fmt --all

fmt-check: ## Check formatting without writing (what CI runs)
	cargo fmt --all --check

lint: ## Clippy, all targets and features, warnings denied
	cargo clippy --all-targets --all-features -- -D warnings

lint-fix: ## Auto-fix what Clippy can, then re-check
	cargo clippy --all-targets --all-features --fix --allow-dirty --allow-staged -- -D warnings

doc: ## Build rustdoc with all features, warnings denied, doctests included
	RUSTDOCFLAGS="-D warnings" cargo doc --all-features --no-deps
	cargo test --doc --all-features

deny: ## Dependency license and advisory gate (cargo-deny)
	cargo deny check licenses advisories

msrv: ## Build and test against the MSRV declared in Cargo.toml (rust-version)
	@if [ -z "$(MSRV)" ]; then echo "could not read rust-version from Cargo.toml"; exit 1; fi
	@echo "MSRV (from Cargo.toml): $(MSRV)"
	rustup toolchain install $(MSRV) --profile minimal --no-self-update
	cargo +$(MSRV) build --all-features
	cargo +$(MSRV) test --all-features

package-check: ## Fail if internal tooling leaks into `cargo package --list`
	@list="$$(cargo package --list --allow-dirty)"; \
	forbidden="$$(printf '%s\n' "$$list" | grep -E '^(\.claude|\.agents|\.codex|rules)/' || true)"; \
	if [ -n "$$forbidden" ]; then \
		echo "internal tooling present in the packaged crate:"; \
		echo "$$forbidden"; \
		exit 1; \
	fi; \
	count="$$(printf '%s\n' "$$list" | wc -l | tr -d ' ')"; \
	echo "package-check: OK ($$count files, no internal tooling)"

check-spanish: ## Fail on Spanish-only characters in Rust comments (repo is English-only)
	@files="$$(git ls-files 'src/*.rs' 'examples/*.rs' | grep -vE '^($(CHECK_SPANISH_EXCLUDE_RE))$$')"; \
	hits="$$(printf '%s\n' "$$files" | xargs rg -n '^[[:space:]]*(///|//!|//)' 2>/dev/null | rg '[$(CHECK_SPANISH_CHARS)]' || true)"; \
	if [ -n "$$hits" ]; then \
		echo "Spanish characters found in comments:"; \
		echo "$$hits"; \
		exit 1; \
	fi; \
	echo "check-spanish: OK"

check: test-default lint fmt-check ## Fast local gate: default tests, lint, fmt-check

pre-push: fmt-check lint test release doc deny msrv package-check check-spanish ## Everything CI runs -- must be green before pushing
	@echo "pre-push: all gates passed"

# release-check is deliberately NOT part of `pre-push` or CI: it asserts the
# version has no pre-release suffix, which is correct only at the moment of
# cutting a release and would fail every ordinary alpha-stage push. See
# RELEASING.md for the full procedure this is one gate of.
release-check: package-check deny ## Verify version/LICENSE/CHANGELOG are release-ready (run only when cutting a release)
	@version="$$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]*)".*/\1/')"; \
	echo "Cargo.toml version: $$version"; \
	case "$$version" in \
		1.*) ;; \
		*) echo "release-check: version '$$version' is not a 1.x release"; exit 1 ;; \
	esac; \
	case "$$version" in \
		*-*) echo "release-check: version '$$version' still carries a pre-release suffix"; exit 1 ;; \
	esac; \
	license="$$(grep -m1 '^license' Cargo.toml | sed -E 's/.*"([^"]*)".*/\1/')"; \
	if [ "$$license" != "MIT" ]; then echo "release-check: Cargo.toml license is '$$license', expected MIT"; exit 1; fi; \
	if [ ! -f LICENSE ]; then echo "release-check: LICENSE file is missing"; exit 1; fi; \
	if ! grep -qi "MIT" LICENSE; then echo "release-check: LICENSE does not look like MIT"; exit 1; fi; \
	if ! grep -qi "GPL" CHANGELOG.md || ! grep -qi "MIT" CHANGELOG.md; then \
		echo "release-check: CHANGELOG.md does not state the MIT relicensing against the GPL history"; exit 1; \
	fi; \
	list="$$(cargo package --list --allow-dirty)"; \
	if ! printf '%s\n' "$$list" | grep -qx "LICENSE"; then \
		echo "release-check: LICENSE is not present in \`cargo package --list\`"; exit 1; \
	fi; \
	echo "release-check: version, license, and packaging OK"

clean: ## Remove build artifacts
	cargo clean
