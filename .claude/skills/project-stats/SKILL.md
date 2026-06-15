---
name: project-stats
description: Use when user asks about project size, lines of code, test coverage, codebase comparison, or "how big is the project"
---

# Project Stats

Report Lucidos project size, test coverage, and comparison to similar projects.

## Counting Rules

**Exclude** `.worktrees/`, `target/`, `node_modules/`, `dist/` — worktrees duplicate the entire codebase and inflate counts 3-4x.

## Commands

Run all in parallel from repo root:

```bash
# Rust lines by module
for dir in crates/lucidos-engine/src/engine crates/lucidos-engine/src/core crates/lucidos-engine/src/llm crates/lucidos-engine/src/memory crates/lucidos-engine/src/runtime crates/lucidos-engine/src/scheduler crates/lucidos-engine/src/api crates/lucidos-engine/src/mcp crates/lucidos-engine/src/bin crates/lucidos-app/src; do lines=$(command find "$dir" -name '*.rs' 2>/dev/null | xargs wc -l 2>/dev/null | tail -1 | awk '{print $1}'); files=$(command find "$dir" -name '*.rs' 2>/dev/null | wc -l | tr -d ' '); echo "$lines $files $dir"; done

# Frontend lines (TS/TSX, CSS separately)
command find ./crates/lucidos-app -name '*.ts' -o -name '*.tsx' | grep -v node_modules | grep -v dist | xargs wc -l 2>/dev/null | tail -1
command find ./crates/lucidos-app -name '*.css' | grep -v node_modules | grep -v dist | xargs wc -l 2>/dev/null | tail -1

# SQL migrations
command find ./crates/lucidos-engine/migrations -name '*.sql' | xargs wc -l 2>/dev/null | tail -1

# Shell scripts
command find ./scripts -name '*.sh' | xargs wc -l 2>/dev/null | tail -1

# Rust test count (total + per module)
grep -r '#\[test\]' --include='*.rs' ./crates | wc -l
for dir in crates/lucidos-engine/src/engine crates/lucidos-engine/src/core crates/lucidos-engine/src/llm crates/lucidos-engine/src/memory crates/lucidos-engine/src/runtime crates/lucidos-engine/src/scheduler crates/lucidos-engine/src/api crates/lucidos-engine/src/mcp crates/lucidos-engine/src/bin crates/lucidos-app/src; do tests=$(grep -r '#\[test\]' --include='*.rs' "$dir" 2>/dev/null | wc -l | tr -d ' '); echo "$tests $dir"; done

# Frontend test count
cd crates/lucidos-app && command npx vitest --run --reporter=verbose 2>&1 | tail -5
```

Use `command find` and `command npx` to bypass rtk hook interception (rtk doesn't support compound find predicates).

## Rust Module Descriptions

| Module | What it does |
|---|---|
| `engine/` | Core orchestrator — chat, agentic loop, Claude Code, tools, event bus, threads |
| `core/` | Events, event store, artifacts, credentials, preferences, backup, email, OAuth |
| `api/` | HTTP routes, SSE, skill UI serving |
| `llm/` | Provider trait, Vertex AI, OpenAI, tool definitions |
| `runtime/` | Python & browser execution |
| `scheduler/` | Cron tasks, notifications, persistence |
| `memory/` | Embeddings, FastEmbed, pgvector index |
| `mcp/` | MCP server integration |
| `bin/` | Test data generators |
| `lucidos-app` | Tauri desktop shell |

## Comparison Benchmarks

Present total **code lines** (Rust + TS/TSX + CSS + SQL + shell, excluding docs/markdown):

| Range | Category | Examples |
|---|---|---|
| 5K-30K | Typical solo/side project | CLI tools, small web apps |
| 30K-100K | Substantial open-source tool | ripgrep (~50K), bat (~30K), delta (~40K) |
| 100K-300K | Mid-size startup product | Series A core product, GitLens (~200K) |
| 300K-1M | Large product | VS Code (~600K), Neovim (~500K) |

## Test Density Assessment

After collecting line counts and test counts, produce a **per-module assessment table** like this:

| Module | Lines | Tests | Ratio | Rating |
|--------|------:|------:|-------|--------|
| `engine/` | 16,029 | 150 | 1:107 | Good |
| `core/` | 9,728 | 0 | — | Needs tests |
| Frontend (TS/TSX) | 26,056 | 687 | 1:38 | Excellent |
| **Overall** | **70K** | **1,059** | **1:66** | **Production-grade** |

### Rating scale

| Ratio | Rating |
|---|---|
| 1:1–1:50 | Excellent |
| 1:50–1:100 | Production-grade |
| 1:100–1:200 | Good |
| 1:200–1:500 | Adequate — room to improve |
| 1:500+ | Needs more coverage |
| 0 tests | Needs tests |

### Rules
- Rate **every Rust module** and the **frontend** individually — don't just report totals
- Modules with **0 tests** always get "Needs tests" regardless of size
- Exclude `bin/` (test data generators) and `lucidos-app` Tauri shell from density assessment — they're not expected to have tests
- After the table, add a **Summary** paragraph: overall assessment, which modules are strongest, which need the most attention, and whether overall density is appropriate for the project's stage
