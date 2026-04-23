#!/bin/bash
# List all CognOS workspaces and their status
# A CognOS workspace has a .cognos/ directory

WORKSPACES_DIR="$HOME/workspaces"

for dir in "$WORKSPACES_DIR"/*/; do
  name=$(basename "$dir")
  cognos_dir="$dir.cognos"
  
  if [ -d "$cognos_dir" ]; then
    ports_file="$cognos_dir/ports"
    if [ -f "$ports_file" ] && [ -s "$ports_file" ]; then
      echo "$name  RUNNING  $ports_file"
    else
      echo "$name  STOPPED"
    fi
  fi
done
