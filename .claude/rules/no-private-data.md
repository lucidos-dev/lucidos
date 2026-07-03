---
globs:
  - "**/*.rs"
  - "**/*.ts"
  - "**/*.tsx"
  - "**/*.md"
  - "**/*.sh"
  - "**/*.json"
  - "**/*.toml"
---

# No Private Data in Shipping Files

**This file is the single source of truth for what counts as private data and how to keep it out of the repo.** `/harden`, `/harden-project`, the `code-review` skill, and the release guard all reference it — do not restate its rules elsewhere, link here. The deterministic patterns it describes live once in [`scripts/lib/private_data_patterns.sh`](../../scripts/lib/private_data_patterns.sh).

## The public-release boundary

Lucidos publishes to the public mirror `github.com/lucidos-dev/lucidos`. The release (`scripts/release-to-lucidos.sh`) sanitizes **only two things**: it drops `docs/plans/**` and the release scripts (`EXCLUDE_PATHS`), and it swaps `WORKSPACES.md` for a generic stub. **Everything else ships verbatim** — `system-knowhow/`, all of `crates/**` *including test fixtures*, `docs/**` (except `docs/plans/`), `scripts/**`, `.claude/**`, and the root docs.

So treat every file outside `docs/plans/**` and `WORKSPACES.md` as **public**. Test fixtures and code comments are shipping code — the rule applies to them exactly as to prose.

## Prohibited (use a generic placeholder instead)

1. **Personal / family data** — real names of family or personal contacts, personal documents (e.g. Norwegian `fullmakt` / `nav-skatt` / `nettbank` references), personal artifact paths (`artifacts/projects/pappa/…`).
2. **The maintainer's name as example/fixture data** — "Kenneth's MacBook", `/Users/kenneth/…`, `kenneth.tiller@…`. (Authorship/attribution is the exception — see below.)
3. **Company-internal identifiers** — org names (`m10s`, `m10s-green`), MDM/infra domains (`*.jamfcloud.com`), internal repo/app/project names used as examples (`user-acquisition`, `momentum-autoresearch`, `ua-analysis`, `ost-jira-workflow`, `ua-backoffice`), colleague names in incidental references.
4. **Machine-specific paths / live workspace names** outside `WORKSPACES.md` — real home paths (`/Users/<realname>`, `/home/<realuser>`), and the named live workspaces `personal` / `work`. Machine specifics belong **only** in `WORKSPACES.md` (which is stubbed at release).

## Approved generic placeholders

| Instead of… | Use |
|---|---|
| an app id / name (`momentum-autoresearch`) | `habit-tracker` / "Habit Tracker" (or `demo-director`) |
| a repo / org (`user-acquisition`, `m10s-green`) | `example-repo` / `example-org` |
| an artifact project (`ua-analysis`, `ost-jira-workflow`, `ua-backoffice`) | `data-analysis`, `ticket-workflow`, `backoffice` |
| a device label ("Kenneth's MacBook"/"iPhone") | "My MacBook" / "My iPhone" (or "Test …") |
| a personal artifact path | `artifacts/projects/reports/cover-letter.pdf` |
| a real home path | `/Users/me`, `/home/u` |
| a live workspace | `~/workspaces/dev` or `~/workspaces/myws` |

## Carve-out: legitimate project-identity attribution is allowed

Authorship and credit are **not** example data and stay as-is:

- The maintainer's full name **"Kenneth Tiller"** in `LICENSE`, `GOVERNANCE.md`, `CODE_OF_CONDUCT.md`, `README.md`, and the `tauri.conf.json` copyright.
- Contributor credit (e.g. "Akram has contributed") in `GOVERNANCE.md`.

The deterministic guard reflects this: bare "Kenneth Tiller" (space form) is not a token, and `akram` is allowlisted in `GOVERNANCE.md`. What IS prohibited is the *possessive/path/email* form (`Kenneth's …`, `/Users/kenneth`, `kenneth.tiller@…`) and `akram` used as a work-item id or incidental reference anywhere else.

## Enforcement (all reference this file — DRY)

- **`/harden`** (per-change) and **`/harden-project`** (whole-project) run an LLM semantic sweep of the diff / tree against this rule — they catch *novel* private data no regex anticipates, with a human in the loop. The `code-review` skill (driven by `/harden`) carries the same check as a review angle.
- **The release guard** `assert_no_private_data` in `scripts/release-to-lucidos.sh` is the fail-closed deterministic floor at the irreversible public push; it sources the patterns above from `scripts/lib/private_data_patterns.sh`.
- **Release knowhow** (the lucidos-ops `release-process.md` recipe, workspace-local) should run a pre-publish semantic sweep over the diff since the last `v*` tag, referencing this rule and naming `assert_no_private_data` as the backstop.

A diff that introduces prohibited data is a `/harden` failure on the same footing as a failing test.
