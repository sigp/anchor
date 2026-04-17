# Anchor Documentation

This directory contains the Anchor client documentation built with [Vocs](https://vocs.dev), a modern documentation framework.

## Structure

```
docs/
├── docs/
│   ├── pages/          # Documentation pages (.mdx files)
│   │   ├── index.mdx   # Landing page
│   │   ├── introduction.mdx
│   │   ├── installation.mdx
│   │   └── ...         # Other documentation pages
│   └── public/         # Static assets (images, etc.)
│       └── anchor-logo.png
├── package.json        # Node.js dependencies
├── vocs.config.ts      # Vocs configuration
└── README.md          # This file
```

## Development

### Prerequisites

- Node.js (latest LTS version)
- npm or yarn
- For Mermaid diagrams: Playwright browsers

### Setup

1. Install dependencies:
   ```bash
   npm install
   ```

2. Install Playwright browsers (needed for Mermaid diagrams):
   ```bash
   npx playwright install
   ```

   On Arch Linux, you may need to install system dependencies first:
   ```bash
   sudo pacman -S icu libxml2 libwebp libffi
   ```

3. Start the development server:
   ```bash
   npm run dev
   ```

4. Open your browser to `http://localhost:5173`

### Building

To build the static site:

```bash
npm run build
```

To preview the built site:

```bash
npm run preview
```

## Contributing

### Adding New Pages

1. Create a new `.mdx` file in `docs/pages/`
2. Add the page to the sidebar configuration in `vocs.config.ts`
3. Write your content using Markdown/MDX syntax

### Editing Existing Pages

All documentation content is in `docs/pages/` as `.mdx` files. Edit these files directly.

### Page Structure

Each page should use this frontmatter structure:

```mdx
---
title: Page Title
description: Page description for SEO
---

# Page Title

Your content here...
```

### Supported Features

- **Markdown**: Standard markdown syntax
- **MDX**: React components in markdown
- **Code blocks**: Syntax highlighting with language support
- **Mermaid diagrams**: Rendered automatically
- **Callouts**: Info, warning, tip boxes using `:::` syntax

### Configuration

The main configuration is in `vocs.config.ts`. This controls:

- Site title and description
- Theme and styling
- Navigation (sidebar and top nav)
- Social links
- Other site metadata

## Troubleshooting

### Mermaid Diagrams Not Rendering

If you see Playwright errors when using Mermaid diagrams:

1. Install Playwright browsers: `npx playwright install`
2. On Linux, install system dependencies as mentioned above

### Pages Not Found

Ensure your sidebar configuration in `vocs.config.ts` matches your file structure in `docs/pages/`.

### Development Server Issues

1. Clear cache: `rm -rf node_modules/.vite`
2. Reinstall dependencies: `rm -rf node_modules && npm install`
3. Restart the dev server: `npm run dev`

## Links

- [Anchor Repository](https://github.com/sigp/anchor)
- [Vocs Documentation](https://vocs.dev)
- [Live Documentation](https://anchor.sigmaprime.io)
