# Anchor Book Documentation Component

## Overview
The Anchor Book is the primary documentation component for the Anchor SSV client, built using mdBook. It provides comprehensive user and developer documentation for this high-performance, secure SSV client written in Rust.

## Purpose
- Serves as the central documentation hub for Anchor users and developers
- Provides installation, usage, and configuration guidance
- Documents advanced features, networking, CLI usage, and development workflows
- Hosts protocol specifications and architectural information

## Key Features
- **User Documentation**: Installation guides, running operators, advanced usage patterns
- **Developer Resources**: Development environment setup, contributing guidelines, protocol specifications
- **Reference Materials**: CLI reference, metrics documentation, FAQs
- **Protocol Documentation**: SSV handshake protocol, system architecture

## Structure
```
book/src/
├── README.md - mdBook project information
├── SUMMARY.md - Book table of contents
├── intro.md - Main introduction page
├── installation.md - Installation instructions
├── running_node.md - Operator running guide
├── advanced.md - Advanced usage topics
├── advanced_networking.md - Network configuration
├── cli.md - CLI command reference
├── metrics.md - Metrics and monitoring
├── faq.md - Frequently asked questions
├── setup.md - Development environment
├── contributing.md - Contribution guidelines
├── developers.md - Developer resources
├── handshake.md - SSV handshake protocol
├── architecture.md - System architecture
└── css/ - Custom styling
```

## Technology Stack
- **mdBook**: Static site generator for technical documentation
- **Markdown**: Content format
- **CSS**: Custom styling for documentation presentation

## Hosting
- Production: [anchor-book.sigmaprime.io](http://anchor-book.sigmaprime.io)
- Source: Maintained in the main Anchor repository under `/book`
- CI/CD: Automatically deployed from the `unstable` branch

## Content Categories

### User Documentation
- Installation and setup procedures
- Operator running instructions
- Advanced configuration options
- Network configuration and troubleshooting

### Developer Documentation
- Development environment setup
- Contributing guidelines and processes
- Code architecture and design decisions
- Protocol specifications and implementations

### Reference Materials
- Complete CLI command documentation
- Metrics and monitoring capabilities
- Common questions and troubleshooting
- API references and examples

## Development Status
The Anchor client is currently under active development and should not be used in production environments. Documentation is continuously updated to reflect the latest features and changes.

## Maintenance
- Open source project with community contributions welcome
- Documentation updates tracked through GitHub issues and pull requests
- Regular synchronization with codebase changes and feature updates