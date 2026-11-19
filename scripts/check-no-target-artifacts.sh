#!/usr/bin/env bash
# Fail if any build output under a `target/` directory is tracked by git.
#
# Rust/Cargo writes build artifacts into `target/` at any directory level, so a
# root-only ignore rule (`/target/`) misses nested crates such as
# clients/rust/target/. Those artifacts bloat the repository and leak developer
# paths. See Astraive/lql#22.
set -euo pipefail

usage() {
  echo "usage: $0 [--self-test] [repo]" >&2
}

self_test() {
  local tmp rc=0
  tmp="$(mktemp -d)"

  git -C "$tmp" init -q
  git -C "$tmp" config user.email test@example.invalid
  git -C "$tmp" config user.name test
  printf 'target/\n' > "$tmp/.gitignore"
  mkdir -p "$tmp/src"
  printf 'fn main() {}\n' > "$tmp/src/main.rs"
  git -C "$tmp" add .gitignore src/main.rs
  git -C "$tmp" commit -qm init

  # A clean tree passes.
  if ! "$0" "$tmp" >/dev/null 2>&1; then
    echo "self-test failed: clean tree rejected" >&2
    rc=1
  fi

  # The nested ignore rule hides a target/ dir below any level.
  mkdir -p "$tmp/clients/rust/target/debug"
  : > "$tmp/clients/rust/target/debug/artifact"
  if ! git -C "$tmp" check-ignore -q clients/rust/target/debug/artifact; then
    echo "self-test failed: nested target/ is not ignored" >&2
    rc=1
  fi

  # Force-tracking an artifact must trip the guard.
  git -C "$tmp" add -f clients/rust/target/debug/artifact
  if "$0" "$tmp" >/dev/null 2>&1; then
    echo "self-test failed: tracked target/ artifact not detected" >&2
    rc=1
  fi

  # Untracking it returns the tree to a clean state.
  git -C "$tmp" rm -r --cached -q clients/rust/target/debug/artifact
  if ! "$0" "$tmp" >/dev/null 2>&1; then
    echo "self-test failed: guard still failing after untrack" >&2
    rc=1
  fi

  rm -rf "$tmp"
  if [[ $rc -eq 0 ]]; then
    echo "self-test: ok"
  fi
  return "$rc"
}

case "${1:-}" in
  --self-test) self_test; exit $? ;;
  -h|--help) usage; exit 0 ;;
esac

repo="${1:-.}"
if ! git -C "$repo" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "error: $repo is not a git repository" >&2
  exit 2
fi

# NUL-safe listing from git, split to lines for matching/display. Path
# components named exactly `target` denote build output directories.
tracked="$(git -C "$repo" ls-files -z | tr '\0' '\n')"
offenders="$(printf '%s\n' "$tracked" | grep -E '(^|/)target/' || true)"

if [[ -n "$offenders" ]]; then
  echo "error: tracked build artifacts found under target/:" >&2
  printf '%s\n' "$offenders" >&2
  echo >&2
  echo "Untrack them with: git rm -r --cached <path>" >&2
  exit 1
fi

echo "ok: no tracked files under target/"
