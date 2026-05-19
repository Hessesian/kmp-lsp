#!/usr/bin/env bash
# pr-review.sh — show all open PRs with their unresolved review comments
#
# Usage: ./contrib/pr-review.sh [--all] [--pr NUMBER]
#
# Without args: lists open PRs with their pending/changes-requested comments.
# --all    : include resolved threads too
# --pr N   : focus on a single PR

set -euo pipefail

REPO_ROOT="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
cd "$REPO_ROOT"

ALL=false
PR_NUM=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --all)  ALL=true; shift ;;
    --pr)   PR_NUM="$2"; shift 2 ;;
    *) echo "unknown arg: $1" >&2; exit 1 ;;
  esac
done

# ── Single PR mode ────────────────────────────────────────────────────────────

show_pr() {
  local num="$1"
  echo "════════════════════════════════════════════════════════════════"
  gh pr view "$num" --json number,title,headRefName,baseRefName,state,reviewDecision \
    --jq '"PR #\(.number): \(.title)\n  Branch: \(.headRefName) → \(.baseRefName)\n  State: \(.state) | Review: \(.reviewDecision // "NONE")"'
  echo ""

  echo "── Review comments ─────────────────────────────────────────────"
  gh pr view "$num" --json reviews --jq \
    '.reviews[] | select(.state != "DISMISSED") | "[\(.state)] \(.author.login): \(.body[:200])"' 2>/dev/null || true

  echo ""
  echo "── Inline comments ─────────────────────────────────────────────"
  gh api "repos/{owner}/{repo}/pulls/${num}/comments" \
    --jq '.[] | "  \(.path):\(.line // "?") [\(.user.login)]: \(.body[:200])"' 2>/dev/null | head -40 || true

  echo ""
}

if [[ -n "$PR_NUM" ]]; then
  show_pr "$PR_NUM"
  exit 0
fi

# ── All open PRs ──────────────────────────────────────────────────────────────

echo "Fetching open PRs…"
PR_NUMBERS=$(gh pr list --state open --json number --jq '.[].number')

if [[ -z "$PR_NUMBERS" ]]; then
  echo "No open PRs."
  exit 0
fi

TOTAL=$(echo "$PR_NUMBERS" | wc -l | tr -d ' ')
echo "Found ${TOTAL} open PR(s)."
echo ""

while IFS= read -r num; do
  # Skip PRs with approved/no changes requested unless --all
  if [[ "$ALL" == false ]]; then
    DECISION=$(gh pr view "$num" --json reviewDecision --jq '.reviewDecision // ""')
    [[ "$DECISION" == "APPROVED" ]] && continue
  fi
  show_pr "$num"
done <<< "$PR_NUMBERS"

echo "Done. Use --all to include approved PRs, --pr N for a single PR."
