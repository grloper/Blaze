#!/usr/bin/env bash
# Fails if the diff vs the PR base deletes tests, adds skip/ignore/xfail markers, or removes
# assertions, or if the total test-case count drops below scripts/test-baseline.txt.
# Bypass: label the PR `test-change-approved`. Refresh baseline: scripts/check-test-integrity.sh --update-baseline
# Env: BASE_REF (default origin/main), PR_LABELS (comma/newline separated; set by CI), EVENT_NAME.
set -euo pipefail
cd "$(dirname "$0")/.."

TEST_RE='(^|/)(tests?|__tests__|spec|e2e)/|(^|/)test_[^/]*$|_test\.[a-z]+$|\.(test|spec)\.[a-z]+$'
# test-case definitions counted for the baseline (any language we use)
CASE_RE='(^|[^A-Za-z_])(def test_|async def test_|(it|test)(\.each)?\(|#\[(tokio::)?test|\[Test\]|\[Fact|\[Theory|\[UnityTest\]|func Test|TestCase\(|describe\()'
BASELINE_FILE="scripts/test-baseline.txt"

count_cases() {
  local n=0 c f
  while IFS= read -r f; do
    [[ -f "$f" ]] || continue
    [[ "$f" =~ $TEST_RE ]] || continue
    c="$(grep -cE "$CASE_RE" "$f" 2>/dev/null || true)"; n=$((n + ${c:-0}))
  done < <(git ls-files)
  echo "$n"
}

if [[ "${1:-}" == "--update-baseline" ]]; then
  count_cases > "$BASELINE_FILE"; echo "baseline updated: $(cat "$BASELINE_FILE")"; exit 0
fi

# Baseline check runs on every event (it needs no PR context).
fail=0
cur="$(count_cases)"
if [[ -f "$BASELINE_FILE" ]]; then
  base_n="$(tr -dc '0-9' < "$BASELINE_FILE")"
  if (( cur < base_n )) && ! printf '%s\n' "${PR_LABELS:-}" | tr ',' '\n' | grep -qx 'test-change-approved'; then
    echo "::error::test-case count dropped: $cur < baseline $base_n"; fail=1
  else echo "test-integrity: test-case count $cur >= baseline $base_n"; fi
else
  echo "::error::missing $BASELINE_FILE (run scripts/check-test-integrity.sh --update-baseline)"; fail=1
fi

if [[ "${EVENT_NAME:-pull_request}" == "pull_request" || "${EVENT_NAME:-}" == "pull_request_target" ]]; then
  if printf '%s\n' "${PR_LABELS:-}" | tr ',' '\n' | grep -qx 'test-change-approved'; then
    echo "test-integrity: label 'test-change-approved' present; diff checks bypassed by owner approval."
    (( fail )) && exit 1; exit 0
  fi
  BASE="${BASE_REF:-origin/main}"
  MB="$(git merge-base "$BASE" HEAD)"
  SELF='^(scripts/check-test-integrity\.sh|\.github/)'

  # 1) deleted (or renamed-away) test files
  while IFS= read -r f; do
    [[ -z "$f" ]] && continue
    if [[ "$f" =~ $TEST_RE ]]; then echo "::error::test file deleted: $f"; fail=1; fi
  done < <(git diff --diff-filter=D --name-only "$MB" HEAD)

  # 2) added skip/ignore/xfail markers (any non-doc file)
  SKIP_RE='(@pytest\.mark\.(skip|skipif|xfail)|pytest\.(skip|xfail)\(|@unittest\.(skip|skipIf|skipUnless|expectedFailure)|\bself\.skipTest\(|#\[ignore|\b(it|test|describe|context)\.(skip|todo|failing)\b|\b(xit|xtest|xdescribe)\(|@Ignore\b|@Disabled\b|\bt\.Skip(Now|f)?\(|\[Ignore|\[Fact\(Skip|Assume\.assume|\.only\(|\[Explicit\]|Assert\.(Ignore|Inconclusive)\()'
  while IFS= read -r f; do
    [[ -z "$f" || "$f" =~ $SELF || "$f" =~ \.(md|txt|rst)$ ]] && continue
    hits="$(git diff -U0 "$MB" HEAD -- "$f" | grep -E '^\+[^+]' | grep -E "$SKIP_RE" || true)"
    if [[ -n "$hits" ]]; then echo "::error::new skip/ignore/xfail marker added in $f:"; echo "$hits"; fail=1; fi
  done < <(git diff --diff-filter=AMR --name-only "$MB" HEAD)

  # 3) removed assertions in test files (more removed than added per file)
  ASSERT_RE='(\bassert\b|\bCheck\(|\bAssert\.[A-Za-z]+\(|\bassert[A-Z_][A-Za-z_]*\(|\bassert_[a-z_]+!?\(|\bassert!\(|\bexpect\(|\bexpect!|\bassertThat\b|\bself\.assert|\bt\.(Error|Fatal)f?\()'
  while IFS= read -r f; do
    [[ -z "$f" ]] && continue
    [[ "$f" =~ $TEST_RE ]] || continue
    d="$(git diff -U0 "$MB" HEAD -- "$f")"
    rem="$(printf '%s\n' "$d" | grep -E '^-[^-]' | grep -cE "$ASSERT_RE" || true)"
    add="$(printf '%s\n' "$d" | grep -E '^\+[^+]' | grep -cE "$ASSERT_RE" || true)"
    if (( rem > add )); then echo "::error::assertions removed in $f (removed=$rem, added=$add)"; fail=1; fi
  done < <(git diff --diff-filter=M --name-only "$MB" HEAD)
else
  echo "test-integrity: not a pull_request event; diff checks skipped."
fi

if (( fail )); then
  echo "test-integrity: FAILED. Add the PR label 'test-change-approved' only if the owner approved this test change."
  exit 1
fi
echo "test-integrity: OK."
