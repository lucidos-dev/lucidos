# No Private Data in Shipping Files

**Always loaded** (no `paths:` frontmatter): this is a safety rule for the public-release boundary and must hold for every file type and every action, including ones that touch no matching path.

**This file is the single source of truth for what counts as private data.** `/harden`, `/harden-project`, the `code-review` skill, and the release guard all reference it: do not restate its rules elsewhere, link here.

## The public-release boundary

Lucidos publishes to the public mirror `github.com/lucidos-dev/lucidos`. The release sanitizes **only two things**: it drops `docs/plans/**` and the release scripts, and it swaps `WORKSPACES.md` for a generic stub. **Everything else ships verbatim**, including all of `crates/**` *with its test fixtures*, `system-knowhow/`, `docs/**`, `scripts/**`, `.claude/**`, and the root docs.

So treat every file outside `docs/plans/**` and `WORKSPACES.md` as **public**. Test fixtures and code comments are shipping code; the rule applies to them exactly as to prose.

## Prohibited (use a generic placeholder instead)

1. **Personal / family data**: real names of family or personal contacts, references to their personal documents (banking, tax, power-of-attorney), personal artifact paths naming a family member.
2. **A contributor's real name as example/fixture data**: a possessive device label ("<Name>'s MacBook"), a home path, an email address. Attribution is the exception, see below.
3. **Company-internal identifiers**: the employer domain and its GitHub org, MDM/infra domains, internal repo/app/project names used as examples, colleague names in incidental references.
4. **Machine-specific paths / live workspace names** outside `WORKSPACES.md`: real home paths (`/Users/<realname>`, `/home/<realuser>`), and the named live workspaces `personal` / `work` *pointed at as machines* (a `~/workspaces/personal` path, "the work workspace has the live data"). The bare words are ordinary English and are fine as UI copy that names no machine: `WORKSPACE_NAME_SUGGESTIONS` in `WorkspacePicker.tsx` offers "personal" / "work" as first-run name suggestions, which says nothing about what exists on this laptop.

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
approved, and one real home path anywhere on the line is a hit. So the natural
"here's the generic form, here's the real one" comment is a leak, not an
illustration. Put the real path nowhere, and let the placeholder carry the
example alone.

## Carve-out: legitimate project-identity attribution is allowed

Authorship and credit are **not** example data and stay as-is: the maintainer's
name as copyright holder (`LICENSE`, the `tauri.conf.json` copyright), as
governance owner (`GOVERNANCE.md`), as code-of-conduct contact
(`CODE_OF_CONDUCT.md`) and in `README.md`; and contributor credit in
`GOVERNANCE.md`.

The guard expresses this as an **exceptions list**: each name is denied outright
and its attribution sites are enumerated beside it. So the same name is a
release-blocking hit anywhere else, as a fixture, a device label, a home path, an
email, a work-item id, or an incidental mention. To add a contributor to the
credits, add the name to the `private-data-exceptions` block in `WORKSPACES.md`
with `GOVERNANCE.md` as its allowed path, never to the credits file alone.

## Enforcement

A diff that introduces prohibited data is a `/harden` failure on the same footing
as a failing test.

- **Semantic**: `/harden` (per-change) and `/harden-project` (whole-project) sweep
  the diff or tree against this rule, catching *novel* private data no regex
  anticipates. The `code-review` skill carries the same check as a review angle.
- **Deterministic**: the release guard in `scripts/release-to-lucidos.sh` is the
  fail-closed floor at the irreversible public push.

The deterministic side is deliberately split so that **no name or private token
appears in any tracked file**: shape heuristics live in
[`scripts/lib/private_data_patterns.sh`](../../scripts/lib/private_data_patterns.sh)
(they describe a form and name nobody), while the enumerated tokens live only in
two marker-fenced blocks in [`WORKSPACES.md`](../../WORKSPACES.md), which the
release stubs. That script's header documents the split, the two block policies,
and both fail-closed arms.
