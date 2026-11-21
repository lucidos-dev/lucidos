# 0368: One definition per value

- **Status**: Accepted
- **Date**: 2026-10-05

## Context

Every Tree memory view budget had its default in two places: the catalog's
`default` string and an `N * 1024` literal in `summary_tree/module.rs`. A unit
test pinned the second copy. Nothing stopped it, and it shipped to main.

A sweep found the same bug across the codebase:

- About 80 Rust preference reads each passed their own fallback. Three
  disagreed with the catalog (the cron timezone, the chat reasoning effort on
  OpenAI routes, and a stale `capture_context` comment).
- Every preference key was written twice: a `PREF_*` const and the catalog.
- About 30 TS sites and eight enum lists copied catalog defaults by hand.
- About 60 more values were restated between Rust and TS, between crates,
  in shell scripts, in knowhow, and in tests.

The plan is `docs/plans/2026-10-05-one-definition-per-value.md`.

## Decision

**One definition per value.** A reader resolves a default from the value's
source and cannot supply its own. A test references the source constant and
never restates its value, unless it pins a wire contract and says so.

Enforcement, strongest first, per class:

| Class | Enforcement |
|---|---|
| Rust preference reads | Typed `Pref<K>` handles in `core/preference_catalog.rs`. A defaulted kind returns `T`, so there is nothing to `unwrap_or`. The string-keyed reads are private. A `const fn` constructor refuses a mismatched spec at compile time. |
| Preference keys | Written once, as `key: "…"` in the catalog. A guard test fails on a key literal anywhere else in Rust. |
| Internal and bookkeeping keys | In the same catalog, marked by `PrefAccess`. `internal_specs()` and `is_silent_key` are views of it, replacing the separate `INTERNAL_KEYS` and `SILENT_PREF_KEYS` lists. |
| Inherited defaults | `PrefDefault::Inherits`, replacing the hand-kept fallback table in `aux_purpose.rs`. |
| TS preference defaults and value lists | Generated `preference-catalog.ts`. `currentPreference(key)` takes no default. A Vitest guard fails on a distinctive default spelled as a scalar, or a second fallback after a read. |
| Other Rust to TS constants | Generated `engine-constants.ts` (engine) and `gateway-constants.ts` (gateway), with staleness tests. |
| Cross-crate copies | Import the owning crate's constant. |
| Shell scripts | One `scripts/lib/workspace_constants.sh`, then table-driven pin tests in the gateway and the engine. |
| Knowhow and README | The engine pin test asserts each doc holds the value rendered from its constant. `preferences.md` defaults are checked by the catalog sync test. |
| Tests restating a constant | Fixed at each site. Enforced by the always-loaded rule and review. |
| Event names in TS | The SSE coverage test fails on a case label that names no event. |

## Rationale

A check catches a copy after someone writes it. An API with no slot for a
default cannot hold one, so it is preferred wherever the type system reaches.
Generation is next: the second language still has a copy, but nobody writes
it. A pin is last, because the copy stays hand-written and only drift fails.

The guards have no allowlist. A hit is fixed at its source. A rule that
defines what counts is not an allowlist, because it names no file. The TS
guard's rule is that an option list is a set of choices, not a default.

## Consequences

- A new preference is one `Pref` const. Its default reaches the engine, the
  agent's tools, the frontend and the doc check with no second edit.
- A stored value the catalog would refuse reads as unset, numbers included:
  an out-of-range number is not a setting at all. `backup_retention`'s bound
  rose to 1000, so a count stored through the old unchecked route still reads.
- Flags share one vocabulary, `FLAG_ON_VALUES` / `FLAG_OFF_VALUES`. A stored
  switch reads it case-insensitively, as it always did. The engine env
  switches that were exact stay exact (`env_switch_is_on`), so
  `LUCIDOS_BIND_ALL=TRUE` still opens nothing.
- An inherited default follows its whole chain. The compactor and memory-find
  models now reach `model_memory`, and the memory-find effort reaches
  `reasoning_memory`, when the keys between are unset. Settings already
  showed the model chain.
- An unset `chat_reasoning_effort` now resolves to the catalog default on every
  route. OpenAI routes used to send no effort.
- The client keeps no copy of the engine's per-family reasoning tiers. With no
  registry answer it offers the whole ladder, and the engine snaps the request.
- The always-loaded set grew by one short rule, `one-definition-per-value.md`.

Four pairs share a number but are different concepts, so they stay apart:

- the event-wait cap and the 24-hour answer wait;
- the backfill batch and the image migration batch;
- the dev port offset base and the engine's fallback port;
- the legacy Postgres image and the current one.

Left as copies, with a reason each, in the plan's report: the TS fallback model
list (it is the net of seed migrations), and the gateway's and CLI's copies of
engine values they cannot import.

## Alternatives considered

- **A guard alone, over string-keyed reads with a default argument.** It
  catches a literal next to a key, but not a default read from a local const.
  The handle API makes both inexpressible.
- **Pinning each TS mirror with a test.** That is what the repo did before.
  It catches drift only for the values someone remembered to pin, and most
  were not. Generation covers every value by construction.
- **Templated knowhow** (`{{const}}` substituted at load). Coding agents and
  the public mirror read the files raw, so the placeholder would be what they
  see. Pins keep the prose readable.
- **A general scan for literals in tests.** A literal in a test cannot be told
  from a restated constant without knowing intent, so the scan would need an
  allowlist. The rule and review carry this class instead.
