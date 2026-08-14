# Canonical Terms

**Always loaded** (no `paths:` frontmatter): the canonical-term rule governs prose, chat replies, and commit messages as well as code, so it cannot be gated on a touched file path.

Lucidos has two glossaries. Use the canonical term from them, never a synonym.

- **`system-knowhow/glossary.md`**: user-facing terms, split into **Core** and **Advanced (coding agents)**. Loaded by the workspace LLM at runtime, and what the UI and the user say.
- **`docs/glossary.md`**: the dev-only extension, for the codebase, design docs and coding-agent sessions.

Read the relevant one when you need a definition. Each is the authority for its own layer, and no term is defined in both.

## Rules

- **Prefer the canonical term.** Names you might be tempted to use but shouldn't:
  - *child thread* used for a transitive descendant → **sub-thread** (a *child thread* is specifically the direct descendant; a *sub-thread* is any descendant).
  - *task* → **intent** (when the user means a goal) or **scheduled task** (when it's a cron job).
  - *recipe* → **knowhow**.
  - *attachment* → **artifact**.
  - *cmd thread / CC subprocess / Claude session* → **coding-agent thread** (or **agent session** for the subprocess + worktree pairing generically; **Claude Code session** / **Codex session** when backend-specific).
  - *AgentKind* → **CodingAgent** (the enum was renamed; variants: `ClaudeCode`, `Codex`).
  - *manifest* used unqualified → pick one: **app manifest** (`manifest.json`), **plugin manifest** (`manifest.toml`), or **signer manifest** (`<name>.manifest.json` sidecar).
  - *command* (for SSE-only events the engine broadcasts) → **transient event** (we use the events-only model; imperative-looking concepts are reframed as past-tense request events like `AppUiRefreshRequested`, never `RefreshAppUI`).
  - *event store* as a banned word → **kept**; the term is valid for the concept (events table + append semantics). What was deleted were the `EventStore` struct's **write** methods, which now live inlined inside *EventBus*; the struct itself remains as a read-only query facade.
- **Same word, same meaning across layers.** A `Trigger` row in DB, a `trigger` directory on disk, a "trigger" in user-facing prose, and the `Trigger` Rust type all refer to the same concept. If you're tempted to use a different word at the boundary, fix the alignment instead.
- **Same name root across DB / Rust / TS / wire / docs / glossary.** Pick one name root per concept and use it literally in every layer that materialises it: a `thread_summaries` table, a `ThreadSummary` Rust struct, a `ThreadSummary` TS interface, the same word in CLI / SDK / LLM-tool docs, and a `thread summary` glossary entry. Each layer keeps its own syntactic convention (plural snake_case for a table, singular PascalCase for a struct); what's banned is the *root* differing (`Info` vs `Summary`). New concept: pick the name once, in the glossary, and write the same root into every layer in the same commit. **A half-renamed concept is worse than the original drift.**
- **A name must describe what the thing is NOW.** The rule above keeps a name consistent across layers; this one keeps it *true* as the thing grows. A name its subject has outgrown misleads hardest for an LLM, which routes on the name rather than by reading the whole file. When you widen something's scope, **rename it in the same change that widened it**, sweeping every surface together: this is no licence for a retroactive sweep, and like the em-dash rule it binds names as you touch them. For an engine-shipped knowhow file the name is also the `load_knowhow` id, so a rename must also reach the engine system prompt's hardcoded routes. Distinct from `CLAUDE.md`'s ban on generic names: that one bans a name which was never specific, this one a name that stopped being accurate.
- **New concept → glossary entry in the same change.** User-meaningful goes in `system-knowhow/glossary.md`, otherwise `docs/glossary.md`. Don't ship a code change that uses a new term without defining it.
- **Renamed or retired → update the glossary in the same commit.** Per `.claude/rules/system-knowhow.md`, `/harden` flags drift between the code surface and the glossary as a hardening failure.

## Where this applies

- **Prose**: `system-knowhow/**/*.md`, `docs/**/*.md`, CLAUDE.md, app/trigger intent files, UI strings, error messages.
- **Code identifiers**: Rust type/field names, TS type/symbol names, DB column names. Naming matters because future readers and the workspace LLM key off these.
- **Commit messages and chat replies**: the same vocabulary the user sees.

## The glossary is a living artifact

It gives a thinking-out-loud conversation a shared vocabulary, and the conversation in turn sharpens it. A design conversation often produces a clearer definition, a sharper distinction, or a new concept worth a name. **Propose the glossary edit in the same turn** rather than filing a TODO. Glossary edits are first-class work, not bookkeeping. The `grill` skill carries the full practice.

If reality has moved, update the entry rather than perpetuating the wrong canonical. That covers a code rename that skipped its doc, and a word the user has settled on in practice. The glossary is normative for *future* writing; when it lags reality, fix it.
