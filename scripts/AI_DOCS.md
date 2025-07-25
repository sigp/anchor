# Scripts Component AI Documentation

## Overview

The `scripts` component contains utility shell scripts for maintaining and managing the Anchor project's documentation and development workflow.

## Core Purpose

This component provides automated tooling for:
- Generating CLI help documentation from the built Anchor binary
- Validating and formatting markdown files using markdownlint

## Key Components

### cli.sh
- **Purpose**: Generates formatted markdown files from CLI help output
- **Key Functions**:
  - `write_to_file()`: Formats CLI help text into markdown with proper headers and code blocks
  - `check()`: Compares existing documentation with newly generated content
- **Output**: Updates help documentation files in the book directory
- **Usage**: Called via `make cli` or `make cli-local` from repository root

### mdlint.sh  
- **Purpose**: Validates markdown formatting across the project
- **Implementation**: Uses Docker container with markdownlint-cli
- **Features**: Automatic fixing of common markdown issues
- **Usage**: Called via `make mdlint` from repository root

## Integration Points

- **Build System**: Integrated with Makefile targets for CI/CD workflows
- **Documentation**: Updates book source files for CLI help
- **Quality Assurance**: Ensures consistent markdown formatting across project

## Dependencies

- Docker (for markdown linting)
- Built Anchor binary (for CLI help generation)
- Standard Unix utilities (diff, sed, etc.)

## Development Notes

- Scripts are designed to be run from repository root via Make targets
- Exit codes indicate whether changes were made or errors occurred
- All generated temporary files are cleaned up after execution