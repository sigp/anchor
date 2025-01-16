.PHONY: tests

GIT_TAG := $(shell git describe --tags --candidates 1)
BIN_DIR = "bin"

X86_64_TAG = "x86_64-unknown-linux-gnu"
BUILD_PATH_X86_64 = "target/$(X86_64_TAG)/release"
AARCH64_TAG = "aarch64-unknown-linux-gnu"
BUILD_PATH_AARCH64 = "target/$(AARCH64_TAG)/release"

PINNED_NIGHTLY ?= nightly

# List of features to use when cross-compiling. Can be overridden via the environment.
# CROSS_FEATURES ?= jemalloc
CROSS_FEATURES ?=

# Cargo profile for Cross builds. Default is for local builds, CI uses an override.
CROSS_PROFILE ?= release

# List of features to use when running CI tests.
TEST_FEATURES ?=

# Cargo profile for regular builds.
PROFILE ?= release

# Extra flags for Cargo
CARGO_INSTALL_EXTRA_FLAGS?=

# Builds the binary in release (optimized).
#
# Binaries will most likely be found in `./target/release`
install:
	cargo install --path anchor --force --locked \
		--features "$(FEATURES)" \
		--profile "$(PROFILE)" \
		$(CARGO_INSTALL_EXTRA_FLAGS)

# The following commands use `cross` to build a cross-compile.
#
# These commands require that:
#
# - `cross` is installed (`cargo install cross`).
# - Docker is running.
# - The current user is in the `docker` group.
#
# The resulting binaries will be created in the `target/` directory.
build-x86_64:
	cross build --target x86_64-unknown-linux-gnu --features "$(CROSS_FEATURES)" --profile "$(CROSS_PROFILE)" --locked
build-aarch64:
	cross build --target aarch64-unknown-linux-gnu --features "$(CROSS_FEATURES)" --profile "$(CROSS_PROFILE)" --locked

# Create a `.tar.gz` containing a binary for a specific target.
define tarball_release_binary
	cp $(1)/anchor $(BIN_DIR)/anchor
	cd $(BIN_DIR) && \
		tar -czf anchor-$(GIT_TAG)-$(2)$(3).tar.gz anchor && \
		rm anchor
endef

# Create a series of `.tar.gz` files in the BIN_DIR directory, each containing
# a `anchor` binary for a different target.
#
# The current git tag will be used as the version in the output file names. You
# will likely need to use `git tag` and create a semver tag (e.g., `v0.2.3`).
build-release-tarballs:
	[ -d $(BIN_DIR) ] || mkdir -p $(BIN_DIR)
	$(MAKE) build-x86_64
	$(call tarball_release_binary,$(BUILD_PATH_X86_64),$(X86_64_TAG),"")
	$(MAKE) build-aarch64
	$(call tarball_release_binary,$(BUILD_PATH_AARCH64),$(AARCH64_TAG),"")

# Runs the full workspace tests in **release**, without downloading any additional
# test vectors.
test-release:
	cargo test --release --features "$(TEST_FEATURES)"

# Runs the full workspace tests in **release**, without downloading any additional
# test vectors, using nextest.
nextest-release:
	cargo nextest run --release --features "$(TEST_FEATURES)"

# Runs the full workspace tests in **debug**, without downloading any additional test
# vectors.
test-debug:
	cargo test --workspace --features "$(TEST_FEATURES)"

# Runs the full workspace tests in **debug**, without downloading any additional test
# vectors, using nextest.
nextest-debug:
	cargo nextest run --workspace --features "$(TEST_FEATURES)"

# Runs cargo-fmt (linter).
cargo-fmt:
	cargo fmt --all -- --check

# Typechecks benchmark code
check-benches:
	cargo check --workspace --benches --features "$(TEST_FEATURES)"


# Runs the full workspace tests in release, without downloading any additional
# test vectors.
test: test-release

# Updates the CLI help text pages in the Anchor book, building with Docker.
cli:
	docker run --rm --user=root \
	-v ${PWD}:/home/runner/actions-runner/anchor sigmaprime/github-runner \
	bash -c 'cd anchor && make && ./scripts/cli.sh'

# Updates the CLI help text pages in the Anchor book, building using local
# `cargo`.
cli-local:
	make && ./scripts/cli.sh

# Check for markdown files
mdlint:
	./scripts/mdlint.sh

# Runs the entire test suite
test-full: cargo-fmt test-release test-debug

# Lints the code for bad style and potentially unsafe arithmetic using Clippy.
# Clippy lints are opt-in per-crate for now. By default, everything is allowed except for performance and correctness lints.
lint:
	cargo clippy --workspace --tests $(EXTRA_CLIPPY_OPTS) --features "$(TEST_FEATURES)" -- \
		-D warnings

# Lints the code using Clippy and automatically fix some simple compiler warnings.
lint-fix:
	EXTRA_CLIPPY_OPTS="--fix --allow-staged --allow-dirty" $(MAKE) lint

# Runs cargo audit (Audit Cargo.lock files for crates with security vulnerabilities reported to the RustSec Advisory Database)
audit: install-audit audit-CI

install-audit:
	cargo install --force cargo-audit

audit-CI:
	cargo audit

# Runs `cargo vendor` to make sure dependencies can be vendored for packaging, reproducibility and archival purpose.
vendor:
	cargo vendor

# Runs `cargo udeps` to check for unused dependencies
udeps:
	cargo +$(PINNED_NIGHTLY) udeps --tests --all-targets --release --features "$(TEST_FEATURES)"

# Performs a `cargo` clean
clean:
	cargo clean

# Check if dependencies are sorted (requires cargo-sort and taplo-cli)
sort:
	cargo sort --check --workspace
	# separate check for root Cargo toml to check workspace dependencies
	taplo fmt -o reorder_keys=true -o "indent_string=    " Cargo.toml --diff --check
