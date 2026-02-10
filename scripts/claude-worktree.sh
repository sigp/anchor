#!/usr/bin/env bash
# Launch Claude Code with worktree-isolated task lists.
#
# Creates a new git worktree (if needed) and launches Claude Code with
# CLAUDE_CODE_TASK_LIST_ID set so each worktree gets its own task list.
#
# Workaround for: https://github.com/anthropics/claude-code/issues/24754
#
# Usage:
#   ./scripts/claude-worktree.sh new <branch> [base]  Create worktree and launch
#   ./scripts/claude-worktree.sh [claude args...]      Launch in current directory
#
# Examples:
#   ./scripts/claude-worktree.sh new feat/my-feature                  # from upstream/unstable
#   ./scripts/claude-worktree.sh new fix/bug-123 origin/stable        # from specific base
#   ./scripts/claude-worktree.sh                                      # in existing worktree
#   ./scripts/claude-worktree.sh --resume                             # with claude args

set -euo pipefail

# --- Helper functions ---

hash_path() {
    local path="$1"
    if command -v sha256sum >/dev/null 2>&1; then
        printf '%s' "${path}" | sha256sum | cut -c1-8
    elif command -v shasum >/dev/null 2>&1; then
        printf '%s' "${path}" | shasum -a 256 | cut -c1-8
    else
        echo "Error: Neither sha256sum nor shasum found in PATH" >&2
        exit 1
    fi
}

launch_claude() {
    local worktree_path="$1"
    shift

    if [ -z "${worktree_path}" ]; then
        echo "Error: Unable to determine working directory" >&2
        exit 1
    fi

    local worktree_dir
    worktree_dir="$(basename "${worktree_path}")"
    local path_hash
    path_hash="$(hash_path "${worktree_path}")"

    export CLAUDE_CODE_TASK_LIST_ID="${worktree_dir}-${path_hash}"

    command -v claude >/dev/null 2>&1 || {
        echo "Error: 'claude' command not found in PATH" >&2
        echo "Please install Claude Code or add it to your PATH" >&2
        exit 1
    }

    echo "Task list isolated to: ${CLAUDE_CODE_TASK_LIST_ID}"
    exec claude "$@"
}

# --- Main ---

if [ "${1:-}" = "new" ]; then
    shift

    if [ $# -lt 1 ]; then
        echo "Usage: $0 new <branch-name> [base-ref]" >&2
        echo "  base-ref defaults to upstream/unstable (fetched automatically)" >&2
        exit 1
    fi

    BRANCH="$1"
    BASE="${2:-upstream/unstable}"

    # Derive worktree directory name from branch (e.g., feat/my-feature -> anchor-my-feature)
    REPO_NAME="$(basename "$(git rev-parse --show-toplevel)")"
    DIR_SUFFIX="$(basename "${BRANCH}")"
    WORKTREE_DIR="../${REPO_NAME}-${DIR_SUFFIX}"

    # Fetch if base looks like a remote ref
    if [[ "${BASE}" == */* ]]; then
        REMOTE="${BASE%%/*}"
        REF="${BASE#*/}"
        echo "Fetching ${REF} from ${REMOTE}..."
        git fetch "${REMOTE}" "${REF}" 2>&1
    fi

    echo "Creating worktree at ${WORKTREE_DIR} on branch ${BRANCH}..."
    git worktree add "${WORKTREE_DIR}" -b "${BRANCH}" "${BASE}"

    WORKTREE_PATH="$(cd "${WORKTREE_DIR}" && pwd)"
    cd "${WORKTREE_PATH}"
    launch_claude "${WORKTREE_PATH}"
else
    # Launch in current directory
    WORKTREE_PATH="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
    launch_claude "${WORKTREE_PATH}" "$@"
fi
