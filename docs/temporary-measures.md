# Temporary measures (the impermanence registry)

The single registry for **things in the codebase that are meant to end and carry a
concrete condition for when.** A temporary measure is anything we added knowing
we'd remove it — a workaround until upstream fixes X, a diagnostic that only
exists to chase a bug, a crutch for a model mistake, a feature flag awaiting
cleanup, a back-compat shim with a real removal trigger. Left untracked, every one
of them silently becomes permanent. This file is where they live so removal stays
an intention with an owner and a trigger, not a someday.

This generalizes the old single-category `model-tolerance-measures.md` (now folded
in as one typed section). Every measure goes here, with a removal condition, the
moment it lands — see [`.claude/rules/temporary-measures.md`](../.claude/rules/temporary-measures.md).

## The inclusion test

For every candidate, ask one line:

> **Is this meant to go away, and is there a concrete condition for when?**

**Yes → it belongs here.** No → it belongs elsewhere (see OUT, below).

## Explicitly OUT (so this registry doesn't become a tech-debt swamp)

These are *not* temporary measures — don't add them here:

- **Permanent back-compat / old-data tolerance** — serde aliases for old events,
  parsers that accept a legacy wire shape, defaults for missing historical fields.
  Real requirements that never leave. (Site-local comment is enough.)
- **Site-local suppressions** — `#[allow(...)]`, `@ts-expect-error`,
  `// eslint-disable`. Harden flags those at the site, not here.
- **Permanent design decisions / accepted non-bugs** — these live in
  [`docs/adr/`](adr/README.md) and [`docs/code-review-priors.md`](code-review-priors.md).
- **General refactor wishlist / "tech debt" with no concrete end condition** —
  that's backlog material / [`docs/plans/`](plans/). If you can't write a concrete
  removal condition, it fails the inclusion test.

## How to read an entry

Every entry — across all four sections — uses the same fields:

| Field | Meaning |
|---|---|
| **Added / Opened** | Date the entry landed. |
| **Lives in** | The file(s) / site that carry the thing. `n/a` for a pure investigation. |
| **Impermanent because** | Why this is meant to end (the model mistake it papers over, the upstream bug it works around, the question it chases) — not a permanent requirement. |
| **Removal / resolution condition** | What concretely has to be true to drop it, and how to verify it's safe. "Eventually" is not a condition. |
| **Status** | `open` / `active` (in place) → `resolved` / `removed` + date. Kept as history — never delete a row. |
| **Investigation** | *(measures only)* the parent-investigation id this measure exists to serve (see § Open investigations). Closing the investigation surfaces every measure now eligible for removal. |

When you remove a measure (or close an investigation), set its status to
`removed` / `resolved`, add the date, and leave the entry as a record — don't
delete the row. Also revert any paired docs/wording the measure softened (the
entry's removal condition should name them).

---

## 1. Temporary measures & workarounds

Diagnostics, scaffolding, and "workaround until upstream fixes X" code.

### iOS-PWA liveness diagnostic

- **Added:** 2026-05-18
- **Lives in:** `crates/lucidos-app/src/utils/liveness.ts` (heartbeat +
  startup-classification + uncaught-error breadcrumbs, posted to the engine's
  `/api/v1/internal/client-log` endpoint).
- **Impermanent because:** Pure telemetry to chase a bug — it detects iOS WKWebView
  content-process crashes that don't fire `pagehide`/`beforeunload`, classifies each
  startup (`likely_crash` / `bg_resume` / `reload_clean` / …), and captures
  window-level errors so the next blackout leaves evidence. It compensates for
  nothing in the design; once the cause is found and fixed, the whole module is dead
  weight.
- **Removal / resolution condition:** When the **iOS-PWA blackout investigation**
  (`ios-pwa-blackout`) is closed — i.e. the root cause of the iOS PWA black-screen /
  silent content-process kill is identified and fixed and the engine.log
  `[Client/lifecycle]` / `[Client/error]` breadcrumbs are no longer needed to
  reproduce or confirm it. Verify by confirming no open work relies on the
  `kind:"likely_crash"` log line, then delete the module and its call sites
  (`startLivenessTracking` / `reportStartupKind`).
- **Status:** active
- **Investigation:** `ios-pwa-blackout`

---

## 2. Model-tolerance measures

A **model-tolerance measure** is a crutch added *only* to compensate for current
LLMs making a predictable, recurring mistake — forgiving where we'd otherwise be
strict, so a weak model's wrong-but-intuitive guess still works. The honest reason
it exists is "the model keeps getting this wrong," not "the design needs it." As
models improve and stop making the mistake, each becomes dead weight to remove.
(This is *not* for genuine ergonomics, backward-compat, or external-robustness —
those stay. The test: *would we still want this if the model were perfect?* No →
it's a tolerance measure.) For these entries, **Impermanent because** names the
exact model mistake the measure tolerates.

### CSS font-variable aliases (`--font-family`, `--font` → `--font-ui`)

- **Added:** 2026-06-25
- **Lives in:** `crates/lucidos-engine/src/api/sdk_iframe.css` (`:root` token block,
  served to every app iframe at `/api/v1/sdk-iframe.css`).
- **Impermanent because (tolerates):** Coding agents building apps reach for
  `var(--font-family)` (it mirrors the CSS *property* name) or the `--font`
  shorthand instead of the canonical `--font-ui`. An undefined `var()` silently
  falls back to the app's hardcoded stack, so the app shipped the system font
  instead of the user's chosen font — with no error. `building-an-app.md` already
  named `--font-family` as a mistake to avoid and it kept recurring, so guidance
  alone was insufficient.
- **Removal / resolution condition:** When app coding-agent threads reliably emit
  `--font-ui` (sample a batch of recently-generated apps; if none reference
  `--font-family` / `--font`, the crutch is dead). On removal, also drop the
  "tolerated aliases" notes in `system-knowhow/js-sdk.md` § Theme variables and
  `system-knowhow/building-an-app.md`, and restore the stricter "don't invent
  `--font-family`" wording.
- **Status:** active

---

## 3. Feature flags & sunset deprecations

Feature flags / kill-switches awaiting cleanup, and back-compat shims/aliases that
carry a **concrete** removal condition (NOT permanent back-compat — that's OUT). A
flag belongs here the moment it lands; a shim belongs here only if you can name the
event that retires it.

_None tracked yet._

---

## 4. Open investigations (parents)

An investigation is the *reason* a measure exists, not a measure itself — so it has
no "Lives in" site of its own. Measures in the sections above reference an
investigation by its id. The payoff: **closing an investigation surfaces every
measure now eligible for removal** — search this file for the id to find them all.

### `ios-pwa-blackout` — iOS PWA black-screen / silent content-process kill

- **Opened:** 2026-05-18
- **Lives in:** n/a (investigation)
- **Impermanent because:** An investigation closes once its question is answered.
  iOS Safari/WKWebView in standalone PWA mode intermittently kills the content
  process (or resumes to a black screen) without firing `pagehide`/`beforeunload`,
  so the failure is invisible to ordinary lifecycle hooks. The diagnostics built to
  chase it (see measures below) exist only until the cause is understood and fixed.
- **Removal / resolution condition:** Root cause identified and fixed, confirmed by
  the absence of fresh `kind:"likely_crash"` breadcrumbs (and no new black-screen
  reports) over a representative iOS-PWA usage window. On close, flip every measure
  tagged `Investigation: ios-pwa-blackout` to `removed` per its own removal steps.
- **Status:** open
- **Measures referencing this investigation:** iOS-PWA liveness diagnostic (§1).
