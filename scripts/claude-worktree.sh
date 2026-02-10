#!/usr/bin/env bash
# Wrapper script to launch Claude Code with worktree-isolated task lists.
#
# Git worktrees share the same .git directory, which causes Claude Code's
# task list (TaskCreate/TodoWrite) to leak across concurrent sessions.
# This script sets CLAUDE_CODE_TASK_LIST_ID based on the worktree directory
# name so each worktree gets its own task list.
#
# Workaround for: https://github.com/anthropics/claude-code/issues/24754
#
# Usage:
#   ./scripts/claude-worktree.sh [claude args...]

set -euo pipefail

# Derive a unique task list ID from the worktree directory name.
# For the main worktree (e.g., "anchor"), this still works — each directory
# gets a consistent, unique ID regardless of whether it's a worktree or not.
WORKTREE_DIR="$(basename "$(git rev-parse --show-toplevel 2>/dev/null || pwd)")"

export CLAUDE_CODE_TASK_LIST_ID="${WORKTREE_DIR}"

echo "Task list isolated to: ${CLAUDE_CODE_TASK_LIST_ID}"
exec claude "$@"
