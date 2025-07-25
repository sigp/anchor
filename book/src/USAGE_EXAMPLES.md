# Anchor Book Usage Examples

## Development Workflow Examples

### Setting up Local Development Environment

```bash
# Install mdBook
cargo install mdbook

# Clone the repository
git clone https://github.com/sigp/anchor.git
cd anchor/book

# Start local development server
mdbook serve --open
```

### Building the Documentation

```bash
# Build static documentation
mdbook build

# Clean previous builds
mdbook clean

# Check for broken links
mdbook test
```

## Content Creation Examples

### Adding a New Documentation Page

1. **Create the markdown file:**
```markdown
# New Feature Documentation

## Overview
This page documents the new feature...

## Installation
```bash
anchor --new-feature enable
```

## Configuration
Add the following to your config:
```toml
[new_feature]
enabled = true
```
```

2. **Update SUMMARY.md:**
```markdown
# Summary

- [Introduction](./intro.md)
- [Installation](./installation.md)
- [New Feature](./new_feature.md)  # Add this line
- [Running an Operator](./running_node.md)
```

### Creating Code Examples

```markdown
# CLI Reference Example

## Basic Commands

### Starting the Anchor Client
```bash
# Start with default configuration
anchor run

# Start with custom config file
anchor run --config /path/to/config.toml

# Start with specific network
anchor run --network mainnet
```

### Key Management
```bash
# Generate new validator keys
anchor keys generate --count 1

# Import existing keys
anchor keys import --keystore /path/to/keystore
```
```

## Content Organization Examples

### User Guide Structure
```markdown
# User Documentation Pattern

## Quick Start
- Installation steps
- Basic configuration
- First run example

## Configuration
- Config file format
- Environment variables
- CLI flags

## Operations
- Starting/stopping
- Health monitoring
- Log management

## Troubleshooting
- Common issues
- Error messages
- Support resources
```

### Developer Guide Structure
```markdown
# Developer Documentation Pattern

## Development Setup
- Environment requirements
- Build instructions
- Testing procedures

## Architecture
- System overview
- Component interactions
- Design decisions

## Contributing
- Code style guidelines
- Pull request process
- Issue reporting
```

## Maintenance Examples

### Updating Documentation for New Release

1. **Update version references:**
```bash
# Find all version references
grep -r "v0.1.0" src/

# Update installation instructions
sed -i 's/v0.1.0/v0.2.0/g' src/installation.md
```

2. **Add release notes:**
```markdown
# Release Notes

## v0.2.0
- Added new consensus mechanism
- Improved network performance
- Fixed memory leak in validator

## v0.1.0
- Initial release
- Basic SSV functionality
```

### Link Validation
```bash
# Check for broken internal links
mdbook test

# Manual link checking
grep -r "\[.*\](.*\.md)" src/ | while read line; do
    file=$(echo $line | cut -d: -f1)
    link=$(echo $line | grep -o '(.*\.md)' | tr -d '()')
    if [ ! -f "src/$link" ]; then
        echo "Broken link in $file: $link"
    fi
done
```

## Advanced Usage Examples

### Custom CSS Integration

```css
/* Custom styling example */
.anchor-highlight {
    background-color: #f0f8ff;
    border-left: 4px solid #0066cc;
    padding: 10px;
    margin: 10px 0;
}

.code-block {
    background-color: #2d3748;
    color: #e2e8f0;
    border-radius: 6px;
    padding: 16px;
}
```

```markdown
<!-- Using custom CSS classes in markdown -->
<div class="anchor-highlight">
This is an important note about Anchor configuration.
</div>
```

### Cross-Reference Examples

```markdown
# Linking Between Pages

See the [installation guide](./installation.md) for setup instructions.

For advanced configuration options, refer to the [CLI reference](./cli.md#configuration).

The [architecture overview](./architecture.md#consensus-layer) explains how consensus works.

## Anchor Links
Jump to specific sections:
- [Network Configuration](./advanced_networking.md#peer-discovery)
- [Metrics Setup](./metrics.md#prometheus-integration)
```

### Code Documentation Integration

```markdown
# API Documentation Example

## Validator Management API

### Create Validator
```http
POST /api/v1/validators
Content-Type: application/json

{
  "public_key": "0x1234...",
  "withdrawal_credentials": "0x5678..."
}
```

**Response:**
```json
{
  "status": "success",
  "validator_id": "12345",
  "activation_epoch": 150000
}
```

### Error Responses
```json
{
  "status": "error",
  "error_code": "INVALID_KEY",
  "message": "Public key format is invalid"
}
```
```

## CI/CD Integration Examples

### GitHub Actions Workflow
```yaml
name: Deploy Documentation

on:
  push:
    branches: [ unstable ]
    paths: [ 'book/**' ]

jobs:
  deploy:
    runs-on: ubuntu-latest
    steps:
    - uses: actions/checkout@v2
    
    - name: Setup mdBook
      run: |
        curl -L https://github.com/rust-lang/mdBook/releases/download/v0.4.21/mdbook-v0.4.21-x86_64-unknown-linux-gnu.tar.gz | tar xz
        sudo mv mdbook /usr/local/bin/
    
    - name: Build book
      run: cd book && mdbook build
    
    - name: Deploy to GitHub Pages
      uses: peaceiris/actions-gh-pages@v3
      with:
        github_token: ${{ secrets.GITHUB_TOKEN }}
        publish_dir: ./book/book
```

### Automated Link Checking
```bash
#!/bin/bash
# check_links.sh

echo "Checking documentation links..."

# Build the book first
mdbook build

# Check for broken internal links
if ! mdbook test; then
    echo "❌ Internal link check failed"
    exit 1
fi

echo "✅ All documentation checks passed"
```

## Performance Optimization Examples

### Image Optimization
```bash
# Optimize images for web
for img in src/images/*.png; do
    optipng -o7 "$img"
done

for img in src/images/*.jpg; do
    jpegoptim --max=85 "$img"
done
```

### Content Minification
```bash
# Minify CSS
cleancss -o book/css/custom.min.css src/css/custom.css

# Optimize HTML output (post-build)
find book -name "*.html" -exec html-minifier --collapse-whitespace {} \;
```