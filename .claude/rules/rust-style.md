---
paths:
  - "**/*.rs"
---

## Rust style

### General
- Use idiomatic `Option`/`Result` and typed errors; avoid stringly-typed designs
- Avoid nested control-flow result shapes like `Option<Result<T, E>>` when a dedicated outcome enum is clearer
- Use clear, descriptive names following Rust conventions (snake_case for functions/variables, CamelCase for types)
- Keep functions/modules focused and small
- Prefer simple solutions first; avoid abstractions without clear current value
- Document public APIs with `///` when introducing or changing public behavior
- Use the type system to prevent errors
- Check requirements first: read existing templates, guidelines, and patterns before implementing

### Async code
- Use `async`/`.await` properly with Tokio
- Handle cancellation correctly
- Avoid blocking the runtime with CPU-intensive work

### Error types
- Create domain-specific error types using `thiserror`
- Include context in errors
- Make error messages user-friendly and actionable

### DRY
- Avoid duplicating logic, validation, or transformation in multiple places
- When the same pattern appears more than once, extract it into a shared function or method

### Single source of truth
- Each piece of domain logic should have one authoritative location, owned by the type or module closest to the data it operates on

### Function readability
- Flag functions that are hard to follow due to deep nesting, excessive length, or mixed abstraction levels
- When a function body exceeds ~50 lines or has more than 2 levels of nesting, extract inner logic into named helper methods
- Prefer flat, linear control flow over deeply indented structures

### Control-flow clarity
- Keep dispatch/selection helpers pure whenever possible (select outcome, do not mutate counters via `&mut`)
- Return explicit outcome enums for multi-branch behavior (`Executed`, `SkippedKnown`, `SkippedUnknown`, etc.)
- Update metrics/counters/logging in the caller, where control flow is easier to read and audit

### Comments
- Comment "why", not "what"
- Use doc comments (`///`) for public API documentation
- Add `TODO`, `FIXME`, or `NOTE` markers as needed
- Use backticks around identifiers in comments: function names (`validate()`), types (`VariableList<u8, U256>`), constants (`RSA_SIGNATURE_SIZE`), field names (`operator_ids`)
