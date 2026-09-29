## Command catalog

Use project Make targets by default.

**Build/install:**
- `cargo build --release` - build in release mode
- `make install` - install to path
- `make build-x86_64` / `make build-aarch64` - cross-compile
- `make build-release-tarballs` - create release archives

**Testing:**
- `make test` - all tests, release mode (standard)
- `make test-debug` - all tests, debug mode
- `make nextest-release` / `make nextest-debug` - nextest runner
- `make test-spec-tests` / `make nextest-spec-tests` - `spec_tests` with `fake_crypto` (full proposer block coverage)
- `cargo test -p <crate>` - specific crate
- `make check-benches` - compile benchmarks without running

**Format & lint:**
- `make cargo-fmt` - format code
- `make cargo-fmt-check` - check formatting
- `make lint` - run clippy
- `make lint-fix` - auto-fix lint issues

**Quality:**
- `make udeps` - unused dependencies
- `make sort` - dependency sort order
- `make audit` - security audit
- `make mdlint` - markdown lint
- `make cli-local` - update CLI docs
