---
name: create-pr
description: Create a GitHub PR with a reviewer-friendly description. Use when the user asks to open a pull request.
allowed-tools: Bash(git *), Bash(gh *), Read, Grep, Glob
---

Create a pull request that is easy for reviewers to understand and evaluate.
Applies GitHub's "helping others review your changes" principles and the
project's PR-shaping rules.

## Pre-flight

1. **Gather state** (run in parallel):
   - `git status` (never use `-uall`)
   - `git diff --stat <base>...HEAD` — file-level summary
   - `git log --oneline <base>...HEAD` — all commits on this branch
   - `git diff <base>...HEAD` — full diff for analysis
   - Read `.github/PULL_REQUEST_TEMPLATE.md` for the required format

   Default base branch: `unstable`. Use `stable` only if the user says so.

2. **Check for uncommitted work.** If there are staged/unstaged changes that
   look like they belong in this PR, ask the user whether to commit first.

3. **Scope check.** If the diff crosses more than 3 crates or exceeds ~200 LOC
   of non-mechanical changes, suggest splitting into a PR stack and stop for
   confirmation.

## Writing the PR

### Title

- Conventional Commits format: `type: short summary` (under 72 chars)
- Types: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`, `perf`, `ci`
- Use `!` for breaking changes: `feat!: changed the API`

### Description — make it easy to review

Follow the section structure from `.github/PULL_REQUEST_TEMPLATE.md` (read in
the pre-flight step) and address each required section. When writing those
sections, weave in these principles:

**1. Lead with motivation ("why before what")**
Open with the problem or goal — what was wrong, missing, or suboptimal.
Reviewers who understand the "why" can evaluate the "how" independently.

**2. Give a mental model of the change**
In 2–4 sentences, describe the shape of the solution at the component level.
Think: "if I could only tell the reviewer one paragraph before they open the
diff, what would make the biggest difference?" Avoid file-by-file inventories.

**3. Guide the reading order**
When the diff touches multiple files, tell the reviewer where to start and
why. Example: "Start with `cli.rs` (struct definitions), then `config.rs`
(mechanical access updates)."

**4. Call out what did NOT change**
Explicitly state preserved behavior, unchanged interfaces, or intentional
non-goals. This prevents reviewers from hunting for phantom regressions.

**5. Flag risks and edge cases honestly**
If there's a tricky part, a known limitation, or a design tradeoff — say so
up front. Reviewers trust authors who surface risks proactively.

**6. Link related context**
Reference issues (`Closes #123`), prior PRs, specs, or discussions that
informed the design. Reviewers shouldn't have to search for context.

**7. State verification done**
List what was run (build, lint, tests) and any manual verification. Don't
just say "tests pass" — say which test suites and whether the change is
covered by existing tests or new ones.

### What to avoid

- Line-by-line narration of the diff (reviewers can read code)
- Implementation details better seen in the code itself
- Redundancy with the commit message or title
- Time estimates or subjective effort commentary

## Posting

1. Push the branch if needed: `git push -u origin <branch>`
2. Create the PR:
   ```
   gh pr create --base <base> \
     --title "<title>" \
     --body "$(cat <<'EOF'
   <body>
   EOF
   )"
   ```
3. Return the PR URL to the user.

## Important

- Never force-push without the user's explicit request.
- Never include `Co-Authored-By` lines unless the user asks.
- If the branch is behind the base, inform the user but do not rebase
  automatically.
- If `gh` auth fails, tell the user to run `gh auth login` and stop.
