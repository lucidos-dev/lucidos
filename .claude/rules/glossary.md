---
globs:
  - "**/*.md"
  - "**/*.rs"
  - "**/*.ts"
  - "**/*.tsx"
---

# Canonical Terms

Lucidos has two glossaries. Use the canonical term from them — never a synonym.

- **`system-knowhow/glossary.md`** — user-facing terms. Split into **Core** (app, trigger, knowhow, intent, script, artifact, app manifest, plugin manifest, signer manifest, thread, child thread, sub-thread, top-thread, spawning thread, parent thread, event, domain event, workspace, plugin, auth module, config, imported, Lucidos Agent, Lucidos Engine, …) and **Advanced — coding agents** (Apply, change, Claude Code, coding agent, coding-agent thread, external-repo coding-agent thread, hardening, repository). Loaded by the workspace LLM at runtime; also what the UI and the user say.
- **`docs/glossary.md`** — dev-only extension (aggregate, actor, ActorMode, agent session, BusEvent, CodingAgent enum, EventBus, event store, EventMeta, MessageOrigin, persisted event, projection, request_id, scheduler blocklist, signer, SystemEvent, ThreadEvent, transient event, worktree, Loadable<T>, …). For the codebase, PRs, design docs, CC sessions.

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
  - *event store* as a banned word → **kept**; the term is valid for the concept (events table + append semantics). What was deleted were the `EventStore` struct's `append` / `append_thread_event` **write** methods — that responsibility now lives inlined inside *EventBus*; the `EventStore` struct itself remains as a read-only query facade.
- **Same word, same meaning across layers.** A `Trigger` row in DB, a `trigger` directory on disk, a "trigger" in user-facing prose, and the `Trigger` Rust type all refer to the same concept. If you're tempted to use a different word at the boundary, fix the alignment instead.
- **Same name across DB / Rust / TS / wire / docs / glossary.** Stronger version of the rule above: pick one name root per concept and use it literally in every layer that materialises it. A `thread_summaries` DB table pairs with a `ThreadSummary` Rust struct, a `ThreadSummary` TS interface, a `ThreadSummary[]` wire response, the same word in CLI / SDK / LLM-tool docs, and a `thread summary` glossary entry. Don't call the same concept `ThreadInfo` in Rust + `Thread` in the SDK + "summary" in the docs — that's drift even though each layer is internally consistent. The DB table can keep its plural-snake-case convention (`thread_summaries`) and the Rust struct keeps its singular-PascalCase (`ThreadSummary`) — those are the same name expressed in each layer's syntactic convention. What's banned is the *root* differing (`Info` vs `Summary`, `Thread` vs `ThreadSummary`). New concept → pick the name once, in the glossary, and write the same root into every layer in the same commit. Renaming after the fact requires touching every surface together — a half-renamed concept is worse than the original drift.
- **New concept → glossary entry in the same change.** If your diff introduces a name that meets a real concept (one we'll reuse in prose), add the entry. If the concept is user-meaningful, add to `system-knowhow/glossary.md`; otherwise `docs/glossary.md`. Don't ship a code change that uses a new term without defining it.
- **Renamed or retired → update glossary same commit.** Per `.claude/rules/system-knowhow.md`, `/harden` flags drift between the code surface and the glossary as a hardening failure.

## Where this applies

- **Prose**: `system-knowhow/**/*.md`, `docs/**/*.md`, CLAUDE.md, app/trigger intent files, UI strings, error messages.
- **Code identifiers**: Rust type/field names, TS type/symbol names, DB column names. *Naming* matters because future readers and the workspace LLM key off these.
- **PR titles / commit messages / chat replies**: same vocabulary the user sees.

## Active use during design dialogue

The glossary is a **living artifact** — it exists to give a thinking-out-loud conversation a shared vocabulary, and the conversation in turn sharpens the glossary. This applies to grilling (`grill`, `grill-me`), brainstorming (`superpowers:brainstorming`), and any back-and-forth where you and the user are pinning down what to build. Three behaviours:

1. **Phrase questions and recommendations in canonical terms.** When asking a multiple-choice question via `AskUserQuestion`, use glossary terms verbatim in the question, the options, and the option descriptions. Don't invent a synonym because it sounds more conversational — the canonical term *is* the conversation. If you find yourself reaching for one, look up the canonical first.
2. **Flag synonym reaches in either direction.** If the user types a synonym you recognize from the "Names you might be tempted to use" list (or that resolves to a glossary entry under a different name), call it out gently inline: *"You said ‘CC subprocess' — the canonical is ‘agent session' (generic) or ‘Claude Code session' (specifically CC); I'll use that going forward."* If you catch yourself doing it, correct yourself before the user has to.
3. **Sharpen the glossary as concepts crystallize.** When a design conversation produces a clearer definition, a sharper distinction, or a brand-new concept worth a name, **propose the glossary edit in the same turn** (don't file it as a TODO). Show the user the proposed entry (or refined wording), get one-click approval via `AskUserQuestion`, then write it. If the conversation reveals an existing entry is vague, do the same — propose the refinement, get sign-off, ship it. Glossary edits are first-class work, not bookkeeping.

The point isn't quizzing — it's that shared vocabulary compounds. Every conversation either reinforces the canonical or improves it. Drift never accumulates.

## When the glossary is wrong

If reality has moved (a code rename happened without a doc update, or the user has settled on a different word in practice), update the glossary entry — don't perpetuate the wrong canonical. The glossary is normative for *future* writing; if it lags behind reality, fix it.
