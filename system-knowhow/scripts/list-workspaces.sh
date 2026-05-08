#!/bin/bash
# List all Lucidos workspaces and their status
# A Lucidos workspace has a .lucidos/ directory

WORKSPACES_DIR="$HOME/workspaces"

for dir in "$WORKSPACES_DIR"/*/; do
  name=$(basename "$dir")
  lucidos_dir="$dir.lucidos"

  if [ -d "$lucidos_dir" ]; then
    ports_file="$lucidos_dir/ports"
    if [ -f "$ports_file" ] && [ -s "$ports_file" ]; then
      echo "$name  RUNNING  $ports_file"
    else
      echo "$name  STOPPED"
    fi
  fi
done
