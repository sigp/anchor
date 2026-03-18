# CLI Documentation — PR Chain Summary

This file tracks the cumulative changes from `unstable` across the phased PR branches.

## PR 1: `refactor: introduce cli crate with semantic help headings`
**Branch:** `pr1/cli-crate-and-headings`

### Changes from `unstable`:

1. **Semantic help headings** (`anchor/client/src/cli.rs`):
   - Replaced `FLAG_HEADER = "Flags"` with per-group heading constants: `SECURITY_OPTIONS`, `EXTERNAL_APIS`, `HTTP_API`, `NETWORK_OPTIONS`, `METRICS_OPTIONS`, `PAYLOAD_BUILDING_OPTIONS`, `ADDITIONAL_OPTIONS`
   - Added `#[command(next_help_heading = ...)]` to each option group struct
   - Moved loose Node fields (`disable_latency_measurement_service`, `operator_dg`, `operator_dg_wait_epochs`, `strict_mfp`) under `ADDITIONAL_OPTIONS`

2. **Logging heading** (`anchor/logging/src/logging.rs`):
   - Added `#[command(next_help_heading = "Logging Options")]` to `FileLoggingFlags`

3. **New crate: `anchor/cli`**:
   - Extracts `Cli`, `AnchorSubcommands`, version statics, and `get_color_style()` from `anchor/src/main.rs`
   - Single source of truth for CLI type definitions
   - `build.rs` ensures `OUT_DIR` is available for `build_profile_name()`

4. **`anchor/src/main.rs`** — Imports from `cli` crate; removed moved items
5. **`anchor/Cargo.toml`** — Added `cli` dep, removed `version` and `ethereum_hashing` (now in `cli`)
6. **`Cargo.toml` (workspace)** — Added `anchor/cli` member and `cli` workspace dependency
