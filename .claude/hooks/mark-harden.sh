#!/bin/bash
# Records this branch's HEAD as hardened in the parent workspace's DB.
# All git inspection + HTTP is in `lucidos hardened mark` (Rust) so this hook
# stays a stable one-liner — never edited when the storage scheme changes.
exec lucidos hardened mark
