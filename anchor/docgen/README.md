# anchor-docgen

A standalone binary that generates CLI reference documentation for Anchor by introspecting the `clap` command tree defined in the `cli` crate.

## Purpose

Anchor's documentation site includes hand-written CLI pages with explanatory prose and examples, plus generated code block snippets derived from the `clap` command tree. `anchor-docgen` generates those snippets directly from the CLI definitions so the reference sections stay accurate without rewriting the full pages.

## Usage

```bash
# Build
cargo build -p docgen

# Print generated CLI reference to stdout (default)
cargo run -p docgen generate

# Update generated reference snippets
cargo run -p docgen update

# Update with a custom docs directory
cargo run -p docgen update --docs-dir path/to/pages

# Check if .mdx files are up to date (useful in CI)
cargo run -p docgen check
```

### Subcommands

| Subcommand | Description |
| --- | --- |
| `generate` | Print generated reference snippets to stdout (default when no subcommand is given) |
| `update` | Write generated reference snippets into `docs/docs/generated/*.mdx` files |
| `check` | Verify generated reference snippets match current CLI definitions; exits non-zero if stale |

## How it works

1. Builds the full `clap::Command` tree from `cli::Cli` via `CommandFactory`. This leverages struct definitions in the `cli` crate.
2. Generates CLI help descriptions using native `clap` functionality for both top-level commands and details grouped by subcommand.
3. Renders each description grouping as a wrapped code block with outputs that emulate an `anchor <command> --help` CLI call.
4. For `update`/`check`, writes and validates generated MDX snippets that are imported by the hand-written CLI pages.

## Generated pages

| CLI command | Output file |
| --- | --- |
| (global options) | `docs/docs/generated/cli-global-options.mdx` |
| `node` | `docs/docs/generated/cli-node-options.mdx` |
| `keygen` | `docs/docs/generated/cli-keygen-options.mdx` |
| `keysplit` | `docs/docs/generated/cli-keysplit-options.mdx` |
