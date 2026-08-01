#!/usr/bin/env bash
# Run Kairo against one AGENT_ISSUES.md prompt (annotaa fixture).
#
# Usage:
#   ./run-issue.sh           # prompt for issue number
#   ./run-issue.sh 3         # issue 3 directly
#   ./run-issue.sh 3 --no-reset
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ISSUES_FILE="${ROOT}/AGENT_ISSUES.md"
KAIRO_DIR="${ROOT}/kairo"
WORKSPACE="${ROOT}/annotaa"

RESET=1
ISSUE_ARG=""

for arg in "$@"; do
  case "$arg" in
    --no-reset) RESET=0 ;;
    -h|--help)
      cat <<'EOF'
Usage: ./run-issue.sh [issue_number] [--no-reset]

Asks for an issue number from AGENT_ISSUES.md (if not passed), optionally
resets annotaa to the buggy baseline, then runs Kairo in this terminal.

  --no-reset   Skip `git reset --hard buggy` in annotaa/
EOF
      exit 0
      ;;
    *)
      if [[ "$arg" =~ ^[0-9]+$ ]]; then
        ISSUE_ARG="$arg"
      else
        echo "Unknown argument: $arg" >&2
        exit 1
      fi
      ;;
  esac
done

if [[ ! -f "$ISSUES_FILE" ]]; then
  echo "Missing issues file: $ISSUES_FILE" >&2
  exit 1
fi
if [[ ! -d "$KAIRO_DIR" ]]; then
  echo "Missing kairo dir: $KAIRO_DIR" >&2
  exit 1
fi
if [[ ! -d "$WORKSPACE" ]]; then
  echo "Missing workspace: $WORKSPACE" >&2
  exit 1
fi

# Parse "### Issue N" + following ``` … ``` prompt blocks into parallel arrays.
mapfile -t ISSUE_NUMS < <(
  awk '
    /^### Issue [0-9]+/ {
      if (match($0, /[0-9]+/)) print substr($0, RSTART, RLENGTH)
    }
  ' "$ISSUES_FILE"
)

if [[ ${#ISSUE_NUMS[@]} -eq 0 ]]; then
  echo "No issues found in $ISSUES_FILE" >&2
  exit 1
fi

prompt_for() {
  local n="$1"
  awk -v want="$n" '
    BEGIN { in_issue=0; in_fence=0; buf="" }
    /^### Issue / {
      in_issue=0; in_fence=0; buf=""
      if (match($0, /[0-9]+/) && substr($0, RSTART, RLENGTH) == want) in_issue=1
      next
    }
    in_issue && /^```/ {
      if (!in_fence) { in_fence=1; next }
      print buf
      exit
    }
    in_issue && in_fence {
      if (buf != "") buf = buf "\n" $0
      else buf = $0
    }
  ' "$ISSUES_FILE"
}

echo "Available issues (from AGENT_ISSUES.md):"
for n in "${ISSUE_NUMS[@]}"; do
  title="$(awk -v want="$n" '
    /^### Issue / {
      if (match($0, /[0-9]+/) && substr($0, RSTART, RLENGTH) == want) {
        getline
        while ($0 ~ /^[[:space:]]*$/) getline
        if ($0 ~ /^```/) { getline; print; exit }
      }
    }
  ' "$ISSUES_FILE" | head -c 72)"
  printf "  %2s) %s…\n" "$n" "$title"
done
echo

if [[ -z "$ISSUE_ARG" ]]; then
  read -r -p "Issue number [1-${ISSUE_NUMS[-1]}]: " ISSUE_ARG
fi

if [[ ! "$ISSUE_ARG" =~ ^[0-9]+$ ]]; then
  echo "Need a numeric issue number." >&2
  exit 1
fi

PROMPT="$(prompt_for "$ISSUE_ARG")"
if [[ -z "${PROMPT// }" ]]; then
  echo "Issue $ISSUE_ARG not found or has an empty prompt." >&2
  exit 1
fi

echo
echo "════════════════════════════════════════════════════════"
echo " Issue $ISSUE_ARG"
echo "════════════════════════════════════════════════════════"
echo "$PROMPT"
echo "════════════════════════════════════════════════════════"
echo

if [[ "$RESET" -eq 1 ]]; then
  echo "[setup] git reset --hard buggy  ($WORKSPACE)"
  git -C "$WORKSPACE" reset --hard buggy
  echo
fi

export RUSTUP_FORCE_ARG0="${RUSTUP_FORCE_ARG0:-cargo}"
# Numbered JSONL under kairo/logs/ when cwd is the crate.
cd "$KAIRO_DIR"

echo "[run] kairo agent — workspace=$WORKSPACE"
echo "[run] logs → $KAIRO_DIR/logs/"
echo

exec cargo run --release --bin kairo -- agent \
  --edit-policy auto \
  --workspace "$WORKSPACE" \
  --task "$PROMPT"
