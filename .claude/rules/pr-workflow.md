## Pull request workflow

### PR title
Must follow [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/) format (enforced by CI):
- `feat:`, `fix:`, `docs:`, `test:`, `chore:`, `perf:`, `refactor:`, `ci:`, `revert:`
- Use `!` for breaking changes (e.g., `feat!: changed the API`)

### PR description
**ALWAYS read `.github/PULL_REQUEST_TEMPLATE.md` first**, then follow the template:
- **Issue Addressed:** Which issue # does this PR address?
- **Proposed Changes:** List or describe the changes introduced
- **Additional Info:** Future considerations or information for reviewers

### Description best practices
- Keep "Proposed Changes" section high-level — focus on what components changed and why
- Avoid line-by-line documentation; use component-level summaries
- Emphasize principles being applied and operational impact
- Focus on the "why", not the mechanics
- Don't mention implementation details (exact files, line numbers, function names)
- Don't state the obvious or repeat information from the title

### Code review culture
Question "why" architectural decisions exist:
- "Why does this intermediary layer exist?"
- "What problem does this abstraction solve?"
- "Could components communicate directly?"
- Working code can still be improved; test passing != design complete
- Question complexity, but respect existing patterns that solve real problems

### PR shaping
- Keep each PR independently reviewable
- Avoid mixing refactor and behavior change unless unavoidable
- Include explicit sections for: Motivation, Scope, Risk, Testing, Rollback
