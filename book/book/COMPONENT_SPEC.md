# Anchor Book - Component Specification

## Technical Specifications

### mdBook Configuration

**File**: `book.toml`
```toml
[book]
language = "en"
multilingual = false
src = "src"
title = "Anchor Book"
author = "Sigma Prime"

[output.html]
additional-css = ["src/css/custom.css"]
default-theme = "coal"
additional-js = ["mermaid.min.js", "mermaid-init.js"]

[preprocessor.mermaid]
command = "mdbook-mermaid"
```

### Content Structure

**Table of Contents** (`src/SUMMARY.md`):
- Introduction and basic concepts
- Installation and setup procedures
- Operator node management
- Advanced configuration options
- CLI reference documentation
- Metrics and monitoring
- Development resources
- Protocol specifications

### File Organization

```
src/
├── README.md              # Book introduction and overview
├── SUMMARY.md             # Navigation structure
├── intro.md              # Introduction to Anchor client
├── installation.md       # Installation instructions
├── running_node.md       # Node operation guide
├── advanced.md           # Advanced usage scenarios
├── advanced_networking.md # Network configuration
├── cli.md                # Command-line interface reference
├── metrics.md            # Monitoring and metrics
├── faq.md                # Frequently asked questions
├── setup.md              # Development environment
├── contributing.md       # Contribution guidelines
├── developers.md         # Developer resources
├── handshake.md          # SSV handshake protocol
├── architecture.md       # System architecture
└── css/
    └── custom.css         # Custom styling
```

### Build Dependencies

**Core Requirements**:
- mdBook (Rust-based documentation generator)
- mdbook-mermaid preprocessor for diagram rendering
- Custom CSS for enhanced visual presentation
- JavaScript integration for interactive features

**Asset Dependencies**:
- FontAwesome for iconography
- Mermaid.js for diagram rendering
- Custom fonts (Open Sans, Source Code Pro)
- Syntax highlighting support

### Output Specifications

**Generated Structure**:
```
book/
├── index.html            # Main entry point
├── [chapter].html        # Individual documentation pages
├── css/                  # Compiled stylesheets
├── fonts/               # Font assets
├── FontAwesome/         # Icon library
├── print.html           # Print-optimized version
├── searchindex.js       # Search functionality
└── static assets        # JavaScript, images, etc.
```

### Configuration Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `language` | String | Documentation language (en) |
| `multilingual` | Boolean | Multi-language support (false) |
| `src` | String | Source directory path |
| `title` | String | Book title displayed in UI |
| `author` | String | Author information |
| `default-theme` | String | UI theme (coal) |
| `additional-css` | Array | Custom stylesheet paths |
| `additional-js` | Array | Custom JavaScript files |

### Preprocessor Configuration

**Mermaid Integration**:
- Command: `mdbook-mermaid`
- Enables diagram rendering in documentation
- Supports flowcharts, sequence diagrams, and architectural diagrams
- JavaScript-based client-side rendering

### Content Standards

**Markdown Extensions**:
- GitHub Flavored Markdown syntax
- Code block syntax highlighting
- Table support
- Link reference system
- Mermaid diagram blocks

**Documentation Structure**:
- Hierarchical organization via SUMMARY.md
- Cross-references between sections
- Consistent formatting and styling
- Comprehensive index and search capability

### Build Process

**Compilation Steps**:
1. Parse `book.toml` configuration
2. Process `src/SUMMARY.md` for navigation structure
3. Convert Markdown files to HTML
4. Apply Mermaid preprocessor for diagrams
5. Integrate CSS and JavaScript assets
6. Generate search index
7. Create cross-reference system
8. Output final HTML documentation

### Deployment Specifications

**Hosting Requirements**:
- Static web server capability
- HTTPS support for secure access
- CDN integration for performance
- Automated deployment from source control

**CI/CD Integration**:
- Source: Git repository (sigp/anchor)
- Branch: unstable (primary development)
- Build trigger: Commit to documentation files
- Output: Static HTML deployment to anchor-book.sigmaprime.io

### Performance Characteristics

**Build Performance**:
- Incremental build support for development
- Fast regeneration during `mdbook serve`
- Optimized asset loading and caching
- Compressed output for production deployment

**Runtime Performance**:
- Client-side search functionality
- Lazy loading of large content sections
- Optimized font and asset delivery
- Responsive design for multiple device types