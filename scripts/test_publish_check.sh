#!/usr/bin/env bash
# Tests scripts/publish_crate.sh with a fake `cargo` so no network is needed.
# Verifies that only an exact-version "already published" response is treated
# as success and that a stale/older published version never hides a failure.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

cat > "$tmp/cargo" <<'FAKE'
#!/usr/bin/env bash
set -euo pipefail
case "${1:-}" in
  publish)
    exit "${FAKE_PUBLISH_EXIT:-0}"
    ;;
  info)
    requested="${@: -1}"
    if [[ "$requested" == "${FAKE_PUBLISHED:-}" ]]; then
      exit 0
    fi
    exit 1
    ;;
  *)
    exit 0
    ;;
esac
FAKE
chmod +x "$tmp/cargo"
export PATH="$tmp:$PATH"

cat > "$tmp/Cargo.toml" <<'TOML'
[package]
name = "lql-client"
version = "0.5.1"
TOML

failures=0
run_case() {
  local name="$1" publish_exit="$2" published="$3" expected="$4"
  local rc
  set +e
  FAKE_PUBLISH_EXIT="$publish_exit" FAKE_PUBLISHED="$published" \
    bash "$repo_root/scripts/publish_crate.sh" "$tmp/Cargo.toml" lql-client \
    >/dev/null 2>&1
  rc=$?
  set -e
  if [[ "$rc" -eq "$expected" ]]; then
    echo "PASS: ${name} (exit ${rc})"
  else
    echo "FAIL: ${name} (expected exit ${expected}, got ${rc})" >&2
    failures=$((failures + 1))
  fi
}

run_case "publish succeeds" 0 "" 0
run_case "publish fails, exact version already published" 1 "lql-client@0.5.1" 0
run_case "publish fails, only older version published (regression)" 1 "lql-client@0.5.0" 1
run_case "publish fails, exact version not published" 1 "" 1

if [[ "$failures" -ne 0 ]]; then
  echo "${failures} publish-check test(s) failed" >&2
  exit 1
fi
echo "All publish-check tests passed"
