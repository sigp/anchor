---
paths:
  - "**/*.rs"
---

## Production safety

- Never use `.unwrap()` or `.expect()` in production paths without explicit safety justification
- Always use proper `Result`/`Option` handling
- Validate user inputs and handle invalid states with typed errors
- Never log secrets or sensitive payloads
- Keep error messages actionable and include context without leaking sensitive data
- Favor memory-safe patterns; avoid `unsafe` unless there is clear necessity and justification
- Leverage Rust's ownership system for memory safety
