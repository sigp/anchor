# Scripts Component Specification

## Component Identity
- **Name**: scripts
- **Type**: Utility Scripts
- **Language**: Bash
- **Location**: `/anchor/scripts/`

## Purpose Statement
Provides automated tooling for documentation generation and markdown validation in the Anchor project development workflow.

## Core Functionality

### CLI Documentation Generation (cli.sh)
```bash
# Function signatures
write_to_file(cmd, file, program)  # Formats CLI output to markdown
check(file, new_file)             # Compares and updates files if changed
```

**Responsibilities**:
- Extract help text from compiled Anchor binary
- Format output as markdown with proper headers and code blocks
- Compare existing documentation with generated content
- Update book source files only when changes detected

**Key Behaviors**:
- Generates temporary markdown files
- Performs diff comparison to detect changes
- Exits with error code if updates are needed
- Cleans up temporary files after execution

### Markdown Linting (mdlint.sh)
```bash
# Docker command execution
docker run markdownlint-cli '**/*.md' --ignore node_modules
```

**Responsibilities**:
- Validate markdown formatting across project
- Automatically fix common formatting issues
- Report validation results with appropriate exit codes

**Key Behaviors**:
- Runs markdownlint in Docker container
- Attempts automatic fixes on validation errors
- Provides clear error reporting and resolution guidance

## Input/Output Specification

### cli.sh
- **Input**: Compiled `./target/release/anchor` binary
- **Output**: Updated markdown files in `./book/src/` directory
- **Side Effects**: May modify existing documentation files

### mdlint.sh
- **Input**: All markdown files in `./book/` directory
- **Output**: Validation report and potential file modifications
- **Side Effects**: May auto-fix markdown formatting issues

## Error Handling
- Scripts exit with code 1 when changes are made or errors occur
- Clear error messages guide users on resolution steps
- Temporary file cleanup is performed regardless of execution outcome

## Dependencies
- Docker runtime (for markdown linting)
- Anchor binary built in release mode
- Standard Unix utilities (diff, sed, rm, cp)

## Integration Requirements
- Must be executed from repository root directory
- Designed for integration with Make build system
- Compatible with CI/CD automation workflows

## Configuration
- No external configuration files required
- Behavior controlled through command-line execution context
- File paths are hardcoded relative to repository root