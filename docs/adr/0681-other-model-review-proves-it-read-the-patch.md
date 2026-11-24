# 0681: The other-model review proves it read the patch

- **Status**: Accepted
- **Date**: 2026-11-13
- **Amends**: ADR 0558

## Context

While hardening a change, the Codex reviewer answered "Unable to assess the
patch: sandbox-exec: sandbox_apply: Operation not permitted".
`scripts/harden-other-model-review.sh` still printed "verdict returned". An
unread patch counted as a completed review.

It was not one bad string. One workspace held 7 Codex review records from the
same day. Four said "Unable to assess" or "Unable to verify" with the same
`sandbox_apply` error, and all 7 printed "verdict returned".

Three causes stacked:

1. **A seatbelt does not nest** (ADR 0645, Rationale). Every Lucidos agent runs
   inside the Lucidos sandbox, so the review script does too.
2. **The Codex plugin's companion forces a nested seatbelt.** Both its review
   paths hard-code `sandbox: "read-only"`, and no flag reaches that choice.
3. **Acceptance was negative.** The Codex adapter accepted any text but one
   generic sentence. The Claude adapter accepted any `findings` array, so
   `{"findings": []}` from a reviewer that read nothing also passed.

The plan is `docs/plans/2026-11-13-other-model-review-proves-it-read-the-patch.md`.

## Decision

A review counts only with positive proof that the reviewer read the patch. The
Codex adapter calls `codex exec` directly on a snapshot, with a sandbox mode
that a probe chooses.

1. **`codex exec`, not the companion.** The adapter runs
   `codex exec -C <snapshot> --sandbox <mode> -c approval_policy="never"
   --ephemeral --json --output-schema <file> -`, with the prompt on stdin. Its
   event stream records every command the reviewer ran, with exit codes.
2. **Codex's sandbox mode comes from a probe.** `codex sandbox -- true` asks
   Codex whether its own sandbox can apply here:
   - it passes: `read-only`;
   - it fails and `LUCIDOS_AGENT_SANDBOXED=1`: `danger-full-access`;
   - it fails with no marker: `unavailable`, exit 2, and the reviewer never
     runs.
3. **A throwaway snapshot.** Each attempt clones the repository with
   `git clone --shared --no-checkout` into the script's temp dir, at the frozen
   head. A write the reviewer makes lands there and is deleted with it.
4. **The range is frozen once.** The script resolves `<base>` and `HEAD` to
   commit ids before the first attempt. Both reviewers review that range, and
   the expected file list comes from it.
5. **One proof check for both reviewers.** The shared schema
   `OMR_FINDINGS_SCHEMA` requires `status`, `summary`, `changed_files` and
   `findings`. An attempt counts only when the reviewer exits 0, its output
   parses as that object, `status` is `reviewed`, and `changed_files` equals
   `git diff --name-only <base>...<head>` as a set.

Anything else is `NO VERDICT`, with one of three reasons on the status line:
`no structured verdict`, `the reviewer said it could not review`, or `its
changed files do not match the diff`.

## Rationale

- **The file list is the proof of read.** A reviewer that could not open the
  diff cannot name its files. A list of "unable" wordings never ends, but a
  positive check does.
- **The probe is the authority.** Codex itself answers whether its sandbox
  applies. No OS name, version or marker alone decides it.
- **Sandbox off is ADR 0645 decision 7.** The engine already runs its own Codex
  sessions with their sandbox off inside the Lucidos sandbox
  (`codex_sandbox_mode`). The outer sandbox wraps the reviewer and every
  descendant, and the reviewer cannot drop it.
- **No approvals, because nobody can answer them.** Decision 7 has Codex ask
  for each command through a card. A headless `codex exec` has no card
  channel, so `never` is the only policy that does not hang. Codex 0.169 also
  rejects `untrusted` in CLI config.
- **Fail closed outside the Lucidos sandbox.** With no marker, nothing would
  bound the reviewer, so it does not run (ADR 0645 decision 8).
- **The snapshot also keeps the review stable.** The hardening agent edits the
  worktree while the review runs. The reviewer reads the frozen head instead.

## Consequences

- Sandboxed agents get real Codex reviews again. A terminal run, or an owner
  who opted out of the agent sandbox, keeps Codex at `read-only`.
- The status line names the failed check and the upstream cause, such as a
  failed command's `sandbox_apply` output.
- The review stays advisory, as ADR 0558 says. `NO VERDICT` and `unavailable`
  never block the hardened marker. They are now truthful.
- **Residual risk, accepted by the owner.** Inside the Lucidos sandbox, the
  Codex reviewer has no write or network ban of its own. The Lucidos sandbox
  still denies `~/.lucidos/private/` and `<workspace>/.lucidos/`, and the env
  scrub removes Lucidos credentials. A prompt-injected reviewer could still
  write elsewhere in `$HOME` or reach the network, as any sandboxed agent can.
- An exact file list can reject a reviewer that names only a subset of a huge
  diff. That fails loudly and retries.
- The Claude reviewer keeps `Read`, `Grep` and `Glob`. It runs no command, so
  no seatbelt nests. It gains the shared schema and the proof check only.
- Reviews from before this change that printed a verdict carry no proof of
  read. They are not re-run.

## Alternatives considered

- **Keep the companion.** It cannot choose a sandbox mode, so inside the Lucidos
  sandbox it always asks for the seatbelt that macOS refuses.
- **Fail closed inside the Lucidos sandbox.** Honest, but it ends Codex review
  for every sandboxed agent, the default. The owner declined it.
- **`codex exec review --base`.** It takes no custom prompt beside `--base`, so
  it cannot ask for the structured verdict the proof needs.
- **Reject outputs that say "unable".** A negative match never ends: the next
  failure uses another wording and passes.
