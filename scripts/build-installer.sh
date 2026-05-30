#!/usr/bin/env bash
# Build platform-native installers locally using the Tauri bundler.
#
# Detects the host OS and invokes `npm --prefix app run tauri build` with the
# appropriate `--bundles` selection:
#   * Linux  -> deb,rpm
#   * macOS  -> dmg (universal binary)
#   * other  -> error
#
# Fails fast on missing toolchains (node, npm, cargo, rustc).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

log() { printf '[build-installer] %s\n' "$*"; }
err() { printf '[build-installer] ERROR: %s\n' "$*" >&2; }

require_cmd() {
  local cmd="$1"
  if ! command -v "$cmd" >/dev/null 2>&1; then
    err "missing required tool: $cmd"
    exit 1
  fi
}

require_cmd node
require_cmd npm
require_cmd cargo
require_cmd rustc

cd "${REPO_ROOT}"

OS_KIND="$(uname -s)"
BUNDLES=""
EXTRA_ARGS=()
case "${OS_KIND}" in
  Linux)
    BUNDLES="deb,rpm"
    ;;
  Darwin)
    BUNDLES="dmg"
    EXTRA_ARGS+=("--target" "universal-apple-darwin")
    log "ensuring rustup targets for universal-apple-darwin are installed"
    rustup target add aarch64-apple-darwin >/dev/null 2>&1 || true
    rustup target add x86_64-apple-darwin >/dev/null 2>&1 || true
    ;;
  MINGW*|MSYS*|CYGWIN*)
    err "Windows hosts should use scripts/build-installer.ps1"
    exit 1
    ;;
  *)
    err "unsupported host OS: ${OS_KIND}"
    exit 1
    ;;
esac

if [[ ! -d "${REPO_ROOT}/app/node_modules" ]]; then
  log "installing frontend dependencies (npm ci)"
  npm --prefix app ci
fi

log "building Tauri bundles: ${BUNDLES}"
npm --prefix app run tauri build -- "${EXTRA_ARGS[@]}" --bundles "${BUNDLES}"

log "produced artifacts:"
ARTIFACT_DIRS=(
  "${REPO_ROOT}/app/src-tauri/target/release/bundle"
  "${REPO_ROOT}/app/src-tauri/target/universal-apple-darwin/release/bundle"
  "${REPO_ROOT}/target/release/bundle"
  "${REPO_ROOT}/target/universal-apple-darwin/release/bundle"
)
FOUND_ANY=0
for d in "${ARTIFACT_DIRS[@]}"; do
  if [[ -d "$d" ]]; then
    while IFS= read -r -d '' f; do
      printf '  %s\n' "$f"
      FOUND_ANY=1
    done < <(find "$d" -type f \( -name '*.deb' -o -name '*.rpm' -o -name '*.dmg' \) -print0)
  fi
done

if [[ "${FOUND_ANY}" -eq 0 ]]; then
  err "no installer artifacts found after build"
  exit 1
fi

log "done."
