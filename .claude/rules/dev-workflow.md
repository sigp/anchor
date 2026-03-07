## Development workflow

### Branches
- Base contributions on `unstable` unless explicitly told otherwise
- `stable`: latest release; `unstable`: development and PR base

### Development sequence
1. Plan scope
2. Implement following project style guidelines
3. Test — add tests that cover changes
4. Document when needed
5. Submit with clean PR metadata

### Pre-commit checks (required)

```bash
make cargo-fmt
make cargo-fmt-check
make lint
make test
```

### Commit messages
- Use present tense ("Add feature", not "Added feature")
- First line is a summary (50 chars or less)
- Include component prefix (e.g., `network:`, `consensus:`)
- Reference issues or tickets when applicable
- Explain why the change was made
- Use Conventional Commits style for PR titles
- Follow `.github/PULL_REQUEST_TEMPLATE.md`
