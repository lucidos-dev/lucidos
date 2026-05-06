#!/bin/bash
# Task 17a: Deterministic skill migration — apps and scripts
#
# Migrates data/skills/ → data/apps/ + data/scripts/ based on structural markers.
# Content splitting (skill.md → prompts + knowhow) is Task 17b (LLM-assisted).
#
# Usage:
#   ./scripts/migrate-skills.sh -w personal
#   ./scripts/migrate-skills.sh -w ~/workspaces/dev
#
# Idempotent: skips already-migrated entries. Does NOT delete data/skills/.

set -euo pipefail

# Parse -w flag (consistent with other scripts)
WORKSPACE=""
while [[ $# -gt 0 ]]; do
    case $1 in
        -w|--workspace) WORKSPACE="$2"; shift 2 ;;
        -h|--help)
            echo "Usage: $0 -w <workspace>"
            echo ""
            echo "Migrates data/skills/ → data/apps/ + data/scripts/"
            echo ""
            echo "Examples:"
            echo "  $0 -w personal"
            echo "  $0 -w ~/workspaces/dev"
            exit 0
            ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

if [[ -z "$WORKSPACE" ]]; then
    echo "Error: No workspace specified."
    echo "Usage: $0 -w <workspace>"
    exit 1
fi

# Bare name → ~/workspaces/<name>
if [[ "$WORKSPACE" != */* ]]; then
    WORKSPACE="$HOME/workspaces/$WORKSPACE"
fi

SKILLS_DIR="$WORKSPACE/data/skills"
APPS_DIR="$WORKSPACE/data/apps"
SCRIPTS_DIR="$WORKSPACE/data/scripts"

if [[ ! -d "$SKILLS_DIR" ]]; then
    echo "No data/skills/ directory found in $WORKSPACE"
    exit 1
fi

PROMPTS_DIR="$WORKSPACE/data/prompts"

mkdir -p "$APPS_DIR" "$SCRIPTS_DIR" "$PROMPTS_DIR"

# Extract YAML frontmatter value — only searches between --- delimiters
frontmatter_value() {
    local file="$1" key="$2"
    awk -v key="$key" '/^---$/{if(n++)exit}n && $0 ~ "^"key":"{gsub(key": *",""); print; exit}' "$file" 2>/dev/null
}

app_count=0
script_count=0
prompt_count=0
skip_count=0

for skill_dir in "$SKILLS_DIR"/*/; do
    [[ -d "$skill_dir" ]] || continue
    id="${skill_dir%/}"
    id="${id##*/}"

    # Apps: ui/ directories → data/apps/
    if [[ -d "$skill_dir/ui" ]]; then
        for ui_component in "$skill_dir/ui"/*/; do
            [[ -d "$ui_component" ]] || continue
            component_name="${ui_component%/}"
            component_name="${component_name##*/}"

            if [[ "$component_name" == "main" ]]; then
                app_id="$id"
            else
                app_id="${id}-${component_name}"
            fi

            if [[ -d "$APPS_DIR/$app_id" ]]; then
                echo "  SKIP APP: $app_id (already exists)"
                skip_count=$((skip_count + 1))
                continue
            fi

            rsync -a --exclude='__pycache__' --exclude='.DS_Store' "$ui_component" "$APPS_DIR/$app_id/"
            echo "  APP: $app_id"
            app_count=$((app_count + 1))
        done
    fi

    # Scripts: scripts/*.py → data/scripts/{id}/
    if [[ -d "$skill_dir/scripts" ]]; then
        if [[ -d "$SCRIPTS_DIR/$id" ]]; then
            echo "  SKIP SCRIPT: $id (already exists)"
            skip_count=$((skip_count + 1))
        else
            # Only migrate if there are actual files to copy
            local_files=()
            for py in "$skill_dir/scripts/"*.py; do
                [[ -f "$py" ]] && local_files+=("$py")
            done
            [[ -f "$skill_dir/scripts/index.json" ]] && local_files+=("$skill_dir/scripts/index.json")

            if [[ ${#local_files[@]} -gt 0 ]]; then
                mkdir -p "$SCRIPTS_DIR/$id"
                cp "${local_files[@]}" "$SCRIPTS_DIR/$id/"
                echo "  SCRIPT: $id"
                script_count=$((script_count + 1))
            fi
        fi
    fi

    # skill.md-based migrations: script-type → data/scripts/, prompt-only → data/prompts/
    if [[ -f "$skill_dir/skill.md" ]]; then
        skill_type=$(frontmatter_value "$skill_dir/skill.md" "type")

        # Script-type skills: run.py → data/scripts/{id}/
        if [[ "$skill_type" == "script" && -f "$skill_dir/run.py" ]]; then
            if [[ -f "$SCRIPTS_DIR/$id/run.py" ]]; then
                echo "  SKIP SCRIPT: $id/run.py (already exists)"
                skip_count=$((skip_count + 1))
            else
                mkdir -p "$SCRIPTS_DIR/$id"
                cp "$skill_dir/run.py" "$SCRIPTS_DIR/$id/run.py"
                echo "  SCRIPT (type:script): $id"
                script_count=$((script_count + 1))
            fi
        fi

        # Prompts: skill.md without ui/ and not a script → data/prompts/
        if [[ "$skill_type" != "script" && ! -d "$skill_dir/ui" ]]; then
            if [[ -f "$PROMPTS_DIR/$id.md" ]]; then
                echo "  SKIP PROMPT: $id (already exists)"
                skip_count=$((skip_count + 1))
            else
                skill_name=$(frontmatter_value "$skill_dir/skill.md" "name")
                body=$(awk '/^---$/{if(n++)found=1;next}found' "$skill_dir/skill.md")
                if [[ -n "$body" ]]; then
                    {
                        echo "---"
                        echo "name: ${skill_name:-$id}"
                        echo "---"
                        echo "$body"
                    } > "$PROMPTS_DIR/$id.md"
                    echo "  PROMPT: $id"
                    prompt_count=$((prompt_count + 1))
                fi
            fi
        fi
    fi
done

echo ""
echo "=== Migration Summary ==="
echo "Apps created:    $app_count"
echo "Scripts created: $script_count"
echo "Prompts created: $prompt_count"
echo "Skipped:         $skip_count"
echo ""
echo "Next steps:"
echo "  1. Verify: ls $APPS_DIR $SCRIPTS_DIR $PROMPTS_DIR"
echo "  2. Run Task 17b (LLM-assisted) to split mixed skill.md → prompts + knowhow"
echo "  3. After full verification, delete data/skills/"
