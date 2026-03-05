---
paths:
  - "**/*.rs"
---

## Testing

**MANDATORY:** Always use the `tester-subagent` via the Task tool before creating or modifying any tests. Do not write test code directly.

### Patterns
- Prefer AAA (Arrange-Act-Assert) structure and clear test naming
- For bug fixes, add a failing repro test before the fix when feasible
- Complex, important, or non-trivial production code must have test coverage
- During review, flag new production functions that lack tests — especially state transitions, side-effect-producing logic, and data-destructive operations

### Database fixtures
- **InMemoryTestFixture**: Fast unit/integration tests using SQLite in-memory databases (`:memory:`). No file I/O overhead; data lost when connection closes.
- **FileTestFixture**: Tests requiring persistence, restart simulation, migrations, or cross-process scenarios. Uses temporary files, automatically cleaned up.
