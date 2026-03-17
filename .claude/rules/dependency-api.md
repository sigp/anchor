---
paths:
  - "**/Cargo.toml"
  - "**/*.rs"
---

## Dependency and API management

**Critical:** Never suggest functionality that doesn't exist in dependencies.

- Check `Cargo.toml` for exact versions before suggesting any dependency APIs
- Verify methods exist in the specific versions used — never assume latest documentation applies
- Read dependency public APIs carefully before recommending features or methods
- Search existing codebase for established patterns, but prioritize best practices over bad existing patterns
- Don't assume capabilities — external dependencies may have architectural constraints
- If you don't see a method in the public API, don't suggest creating or using it
- When uncertain, check dependency source code or ask instead of guessing
- Don't extrapolate — don't assume dependencies support common patterns if they have different design requirements
- For dependency upgrades, verify feature-gated API changes before refactors
- Prefer checked boundary conversions over unchecked casts
- Keep dependencies minimal and up to date
- Prefer well-maintained crates; pin versions appropriately
