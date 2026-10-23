#!/usr/bin/env bash
# Write every generated file. They are build outputs: the Rust-rendered TS,
# CSS and JSON under the three `generated/` folders, then the
# SDK bundles in packages/lucidos-sdk/dist/. Every consumer runs this first.
# `--help` lists the modes; scripts/lib/codegen.sh documents the cache.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/codegen.sh
source "$ROOT/scripts/lib/codegen.sh"

codegen_main "$ROOT" "$@"
