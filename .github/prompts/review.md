# Code Review Guidelines

Only comment on issues you are CONFIDENT are real problems:

1. **Security** — vulnerabilities, unsafe code, input validation, auth logic
2. **Correctness** — logic errors, race conditions, edge cases, off-by-one errors
3. **Performance** — bottlenecks, unnecessary allocations, resource leaks
4. **Error Handling** — missing error paths, unwrap/expect in non-test code, silent failures
5. **Rust Idioms** — non-idiomatic patterns, unnecessary clones, misuse of ownership/borrowing
6. **Design** — incorrect abstractions, module boundary violations, missing trait bounds
7. **Testing** — missing coverage for new code paths, untested edge cases

Do NOT comment on:
- Style, formatting, naming (handled by rustfmt/clippy)
- Documentation, TODOs, FIXMEs
- Pre-existing issues not introduced by this PR
- Nice-to-have suggestions or minor improvements
- Rust idiom preferences that don't affect correctness
- Code with lint suppression comments (already acknowledged)

When you DO find issues:
- Use inline comments with concrete fix suggestions
- Post each inline comment as soon as the issue is confirmed; do not save
  them all up for the end of the review
- Post a brief summary comment ONLY listing the issues found
- No preamble, no praise, no filler
