## Verification recipe (mandatory)

Every completion must include:
- Formatting check
- Lint check
- Tests (or an explicit reason tests were not run)
- Evidence for behavior claims (command output, reproduction, or source reference)

If behavior is not verified, label it as a hypothesis instead of a fact.

### Default commands

```bash
make cargo-fmt && make cargo-fmt-check
make lint
make test
```

### Additional quality checks
- Consider performance implications
- Ensure backwards compatibility when applicable
- Run `make audit` when dependencies change
