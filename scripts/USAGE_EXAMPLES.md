# Scripts Component Usage Examples

## CLI Documentation Generation

### Basic Usage
```bash
# From repository root
make cli

# Alternative local execution
make cli-local
```

### Manual Script Execution (Not Recommended)
```bash
# Must be run from repository root
cd /path/to/anchor
./scripts/cli.sh
```

### Expected Output
```bash
# When no changes needed
CLI help texts are up to date.

# When documentation is updated
./book/src/help_general.md has been updated
Exiting with error to indicate changes occurred. To fix, run 'make cli-local' or 'make cli' and commit the changes.
```

## Markdown Linting

### Basic Usage
```bash
# From repository root
make mdlint
```

### Manual Script Execution (Not Recommended)
```bash
# Must be run from repository root  
cd /path/to/anchor
./scripts/mdlint.sh
```

### Expected Output
```bash
# When all files are properly formatted
All markdown files are properly formatted.

# When formatting issues are found
Exiting with errors. Run 'make mdlint' locally and commit the changes.
# (Script will attempt automatic fixes)

# When serious errors occur
Exiting with exit code >1. Check for the error logs and fix them accordingly.
```

## Integration with Development Workflow

### Pre-commit Checks
```bash
# Ensure documentation is current
make cli

# Validate markdown formatting
make mdlint

# Both should exit with code 0 for clean builds
```

### CI/CD Pipeline Integration
```bash
# In CI scripts
make cli
if [ $? -ne 0 ]; then
    echo "CLI documentation needs updating"
    exit 1
fi

make mdlint  
if [ $? -ne 0 ]; then
    echo "Markdown formatting issues found"
    exit 1
fi
```

### Development Best Practices
```bash
# After making CLI changes, update docs
cargo build --release
make cli
git add book/src/help_*.md
git commit -m "Update CLI documentation"

# Before committing markdown changes
make mdlint
# Fix any remaining issues manually if needed
git add .
git commit -m "Fix markdown formatting"
```

## Common Scenarios

### Updating CLI Help Text
1. Modify CLI argument parsing in source code
2. Build release binary: `cargo build --release`
3. Generate updated docs: `make cli`
4. Commit changes to generated markdown files

### Fixing Markdown Issues
1. Run linting: `make mdlint`
2. Review any manual fixes needed
3. Re-run linting to verify: `make mdlint`
4. Commit formatting changes

### Troubleshooting

#### Script Fails to Find Binary
```bash
# Ensure binary is built
cargo build --release
ls -la target/release/anchor
```

#### Docker Issues with Markdown Linting
```bash
# Verify Docker is running
docker --version
docker run hello-world
```

#### Permission Issues
```bash
# Ensure scripts are executable
chmod +x scripts/cli.sh scripts/mdlint.sh
```