#!/usr/bin/env bash
# Wrapper script to launch Claude Code with worktree-isolated task lists.
#
# Git worktrees share the same .git directory, which causes Claude Code's
# task list (TaskCreate/TodoWrite) to leak across concurrent sessions.
# This script sets CLAUDE_CODE_TASK_LIST_ID based on the worktree directory
# so each worktree gets its own task list.
#
# Workaround for: https://github.com/anthropics/claude-code/issues/24754
#
# Usage:
#   ./scripts/claude-worktree.sh [claude args...]

set -euo pipefail

# Derive a unique task list ID from the full worktree path.
# Using a hash ensures different directories with the same basename get unique IDs.
WORKTREE_PATH="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
WORKTREE_DIR="$(basename "${WORKTREE_PATH}")"
if command -v sha256sum >/dev/null 2>&1; then
    PATH_HASH="$(echo "${WORKTREE_PATH}" | sha256sum | cut -c1-8)"
elif command -v shasum >/dev/null 2>&1; then
    PATH_HASH="$(echo "${WORKTREE_PATH}" | shasum -a 256 | cut -c1-8)"
else
    echo "Error: Neither sha256sum nor shasum found in PATH" >&2
    exit 1
fi

export CLAUDE_CODE_TASK_LIST_ID="${WORKTREE_DIR}-${PATH_HASH}"

# Validate that claude command exists
command -v claude >/dev/null 2>&1 || {
    echo "Error: 'claude' command not found in PATH" >&2
    echo "Please install Claude Code or add it to your PATH" >&2
    exit 1
}

echo "Task list isolated to: ${CLAUDE_CODE_TASK_LIST_ID}"
exec claude "$@"
