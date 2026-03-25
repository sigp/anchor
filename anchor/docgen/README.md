# anchor-docgen

A standalone binary that generates CLI reference documentation for Anchor by introspecting the `clap` command tree defined in the `cli` crate.

## Purpose

Anchor's documentation site includes CLI reference pages (`.mdx` files) describing CLI flags and options in structured markdown tables. `anchor-docgen` derives them directly from the `clap` structs and outputs structured help documentation, enabling automated maintenance of project documentations and consistency checks.

The tool renders grouped option tables in MDX-compatible markdown and can inject generated content into existing `.mdx` files between markers (`{/* CLI_REFERENCE_START */}` / `{/* CLI_REFERENCE_END */}`). This allows additional manually maintained sections to remain in the documentation.

## Usage

```bash
# Build
cargo build -p docgen

# Print generated CLI reference to stdout (default)
cargo run -p docgen

# Update .mdx files in place
cargo run -p docgen -- update

# Update with a custom docs directory
cargo run -p docgen -- update --docs-dir path/to/pages

# Check if .mdx files are up to date (useful in CI)
cargo run -p docgen -- check
```

### Subcommands

| Subcommand | Description |
| --- | --- |
| `generate` | Print raw CLI reference content to stdout (default when no subcommand is given) |
| `update` | Write generated content into `.mdx` files between explicit markers |
| `check` | Verify `.mdx` files match current CLI definitions; exits non-zero if stale |

## How it works

1. Builds the full `clap::Command` tree from `cli::Cli` via `CommandFactory`. This leverages struct definitions in the `cli` crate.
2. Groups arguments by their `clap` `ArgGroup` (derived from `#[derive(Parser)]` structs).
3. Renders each group as a markdown table with Option, Description, and Default columns.
4. For `update`/`check`, splices generated content between explicit markers in the target `.mdx` files, preserving surrounding prose.

## Generated pages

| CLI command | Output file |
| --- | --- |
| (global options) | `cli.mdx` |
| `node` | `cli-node.mdx` |
| `keygen` | `cli-keygen.mdx` |
| `keysplit` | `cli-keysplit.mdx` |
