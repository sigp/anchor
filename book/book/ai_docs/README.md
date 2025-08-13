# Book Documentation Component

## Overview
This component contains the generated documentation book for the Anchor SSV client. It's built using mdBook and contains HTML, CSS, JavaScript, and assets for the documentation website.

## Structure
- **HTML files**: Pre-built documentation pages (installation.html, architecture.html, etc.)
- **CSS/**: Stylesheets for the book theme and custom styling
- **FontAwesome/**: Icon fonts and related assets
- **JavaScript**: Interactive features, search functionality, and syntax highlighting
- **src/**: Source markdown files used to generate the book

## Purpose
Provides user-facing documentation for:
- Installation and setup instructions
- Architecture overview
- CLI usage
- Development guidelines
- FAQ and troubleshooting

## Key Files
- `index.html`: Main entry point for the documentation
- `book.js`: Core book functionality
- `searchindex.js`: Search index for documentation
- `src/SUMMARY.md`: Table of contents structure
- `book.toml`: mdBook configuration

## Build Process
This is a generated component created by mdBook from markdown sources in the `src/` directory.