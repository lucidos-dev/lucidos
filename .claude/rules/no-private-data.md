# No Private Data in Shipping Files

**Always loaded** (no `paths:` frontmatter): this is a safety rule for the public-release boundary and must hold for every file type and every action, including ones that touch no matching path.

**This file is the single source of truth for what counts as private data and how to keep it out of the repo.** `/harden`, `/harden-project`, the `code-review` skill, and the release guard all reference it — do not restate its rules elsewhere, link here.

The deterministic side is split in two, and the split is itself a rule:

- **Shape heuristics** (real home paths under **both** roots — `/Users/<name>` and `/home/<name>` — and possessive device labels) live once in [`scripts/lib/private_data_patterns.sh`](../../scripts/lib/private_data_patterns.sh) — tracked, and public like everything else. They describe a *form*, and name nobody.
- **The enumerated tokens** — contributor names, employer domain, MDM domain, internal project names, personal-document words, family names — live *only* in two marker-fenced blocks in [`WORKSPACES.md`](../../WORKSPACES.md), which the release stubs: `private-data-denylist` (denied everywhere) and `private-data-exceptions` (denied everywhere except the listed attribution sites). A tracked patterns file that spelled any of them out would itself be the leak it exists to prevent, so **no name or private token appears in this document, in the patterns file, or in its test — only categories.** The patterns file loads both blocks at scan time and fails closed if either is missing or malformed.

## The public-release boundary

Lucidos publishes to the public mirror `github.com/lucidos-dev/lucidos`. The release (`scripts/release-to-lucidos.sh`) sanitizes **only two things**: it drops `docs/plans/**` and the release scripts (`EXCLUDE_PATHS`), and it swaps `WORKSPACES.md` for a generic stub. **Everything else ships verbatim** — `system-knowhow/`, all of `crates/**` *including test fixtures*, `docs/**` (except `docs/plans/`), `scripts/**`, `.claude/**`, and the root docs.

So treat every file outside `docs/plans/**` and `WORKSPACES.md` as **public**. Test fixtures and code comments are shipping code — the rule applies to them exactly as to prose.

## Prohibited (use a generic placeholder instead)

1. **Personal / family data** — real names of family or personal contacts, references to their personal documents (banking, tax, power-of-attorney), personal artifact paths naming a family member.
2. **A contributor's real name as example/fixture data** — a possessive device label ("<Name>'s MacBook"), a home path, an email address. (Attribution is the exception — see below.)
3. **Company-internal identifiers** — the employer domain and its GitHub org, MDM/infra domains, internal repo/app/project names used as examples, colleague names in incidental references. (The concrete tokens are the denylist in `WORKSPACES.md`.)
4. **Machine-specific paths / live workspace names** outside `WORKSPACES.md` — real home paths (`/Users/<realname>`, `/home/<realuser>`), and the named live workspaces `personal` / `work`. Machine specifics belong **only** in `WORKSPACES.md` (which is stubbed at release).

## Approved generic placeholders

| Instead of… | Use |
|---|---|
| an internal app id / name | `habit-tracker` / "Habit Tracker" (or `demo-director`) |
| an internal repo, or the employer's GitHub org | `example-repo` / `example-org` |
| an internal artifact project | `data-analysis`, `ticket-workflow`, `backoffice` |
| a device label naming a real person | "My MacBook" / "My iPhone" (or "Test …") |
| a personal artifact path | `artifacts/projects/reports/cover-letter.pdf` |
| a real home path | `/Users/me`, `/home/u`, `/home/user` |
| a live workspace | `~/workspaces/dev` or `~/workspaces/myws` |

**Placeholders excuse themselves, not their line.** The home-path allow-list is
applied per **occurrence**: a line is clean only when EVERY home path on it is
an approved placeholder, and one real home path anywhere on the line is a hit.
So the natural "here's the generic form, here's the real one" comment —
`/Users/me/ws vs the real /Users/<realname>/ws` — is a leak, not an
illustration; put the real path nowhere, and the placeholder alone carries the
example. (A per-*line* filter would drop that whole line as approved and ship
the real path — the masking gap fixed on 2026-07-29.)

## Carve-out: legitimate project-identity attribution is allowed

Authorship and credit are **not** example data and stay as-is: the maintainer's
name as copyright holder (`LICENSE`, the `tauri.conf.json` copyright), as
governance owner (`GOVERNANCE.md`), as code-of-conduct contact
(`CODE_OF_CONDUCT.md`) and in `README.md`; and contributor credit in
`GOVERNANCE.md`.

The deterministic guard expresses this as the **exceptions list**: each name is
denied outright and its attribution sites are enumerated beside it, in the
`private-data-exceptions` block of `WORKSPACES.md`. So the same name is a
release-blocking hit anywhere else — as a fixture, a device label, a home path,
an email, a work-item id, or an incidental mention. (Narrowing the token to
match only the possessive/path form would have been the alternative, but that
silently permits every other bare use.)

To add a contributor to the credits, add the name to the exceptions block with
`GOVERNANCE.md` as its allowed path — never to the credits file alone.

## Enforcement (all reference this file — DRY)

- **`/harden`** (per-change) and **`/harden-project`** (whole-project) run an LLM semantic sweep of the diff / tree against this rule — they catch *novel* private data no regex anticipates, with a human in the loop. The `code-review` skill (driven by `/harden`) carries the same check as a review angle.
- **The release guard** in `scripts/release-to-lucidos.sh` is the fail-closed deterministic floor at the irreversible public push; it sources `scripts/lib/private_data_patterns.sh` and calls `private_data_grep_tree` on the exact tree about to be published. Fail-closed has two arms: a hit refuses the release, and so does a denylist that won't load — a disarmed guard must never read as "clean".
- **Release knowhow** (the lucidos-ops `release-process.md` recipe, workspace-local) should run a pre-publish semantic sweep over the diff since the last `v*` tag, referencing this rule and naming `assert_no_private_data` as the backstop.

A diff that introduces prohibited data is a `/harden` failure on the same footing as a failing test.
