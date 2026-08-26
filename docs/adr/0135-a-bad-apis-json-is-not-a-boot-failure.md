# 0135: A bad apis.json entry is rejected per provider, not a boot failure

- **Status**: Accepted
- **Date**: 2026-08-26

## Context

`data/config/apis.json` configures the credentialed proxy. The engine used to
validate it at startup and refuse to boot on any error, with the reasoning
written at the call site: better to refuse to start than to silently lose proxy
auth.

A live workspace was offline for five hours under that rule. Its chat agent had
appended one entry in the pre-pipeline `auth.type` shape beside three entries
already in the pipeline shape. The startup migration judged the whole file
migrated, because it saw a pipeline sibling first. The load then failed, and the
boot aborted on every restart. The picker offered only "engine failed to become
healthy after repeated restarts", while the actionable line sat in a log file.

Two properties of this file make the fail-closed rule wrong rather than merely
unlucky:

- **The agent writes it.** It is not an operator-curated file edited once at
  install. Every workspace whose agent adds a proxy entry can produce this.
- **The only repair is outside the product.** The UI is the thing that is down,
  so there is no screen to fix the file from. That is not a proportionate
  response to a typo in one entry.

## Decision

An `apis.json` problem never stops the engine booting. Every entry is parsed on
its own: the good ones load, the rejected ones are named with a reason, and the
workspace is told through `SystemEvent::ProxyConfigRejected`. A request against
a rejected name answers **502** carrying that reason.

Only a failure classified unrecoverable may still abort a boot. Today that means
the newer-database case in `boot_failure.rs`, which no retry can fix.

## Rationale

**The blast radius has to match the fault.** One misconfigured provider is a
fault in one provider. Taking the workspace's threads, apps, triggers and chat
down with it is orders of magnitude larger than the thing that broke.

**Fail-closed protects a real thing, and we keep it where it belongs.** The
worry was silently losing proxy auth, and the answer is a 502 rather than a
boot abort. A rejected provider fails at the point of use, loudly, naming
the config error. Nothing degrades to unauthenticated.

**A rejection is not a 404, and the difference is security-relevant.** A 404 from
`resolve_proxy_target` falls through to the builtin model-provider of the same
name (`proxy_builtin.rs`). If a rejected `openai` entry answered 404, its traffic
would silently go to `api.openai.com` instead of the override the user
configured. So a rejected name answers 502, and a file-level rejection answers for
every name, builtins included: an unreadable file may have overridden one, and
there is no way left to know which.

**Visibility is the load-bearing half.** Removing the abort without adding a
surface would trade a loud wedge for a silent one.

**And an SSE event is not that surface, on this code path.** The read happens
before the database is up, so the earliest announce is still hundreds of lines
before `axum_server::bind`. `/api/v1/events` hands a client the live broadcast
with no replay, so an event emitted there reaches zero subscribers on every
ordinary boot. The guaranteed surface is therefore a **notification**, which the
bus projects into `notifications` and the client reads whenever it connects. The
`ProxyConfigRejected` event is emitted beside it for the timeline and for
triggers, and its toast catches the one case the notification does not need: a
page reconnecting through a restart the user is watching.

## Consequences

- A workspace with a broken `apis.json` boots, and everything except the
  rejected providers works.
- Every rejection is announced once per boot, and named at the point of use.
- **A rejection introduced while the engine runs is not announced.** The load
  is per request and the announce is per boot. So an entry the agent appends to
  a live workspace surfaces only as the 502, which does reach the caller: an
  app's fetch, or the `proxy_request` tool's error in the thread. The next boot
  announces it. Closing that gap needs a re-announce keyed on the load result
  changing, which is its own change.
- A file-level parse failure disables the builtin model proxies too, for as
  long as the file stays unreadable. That is the conservative direction, and it
  is now visible rather than silent.
- The startup migration no longer aborts either. It upgrades what it can and
  leaves what it cannot, and the load reports the remainder using the
  translator's own words rather than serde's. It also takes its backup only
  once a rewrite is certain. A file of only untranslatable entries therefore
  stops copying and deleting one on every boot.
- The generic gap stays open: an unclassified fatal boot abort still tells the
  picker nothing. `apis.json` is no longer one of those aborts, so this ADR
  does not widen `boot_failure::report`. See "Alternatives considered".

## Alternatives considered

**Keep the boot abort but report it through `boot_failure::report`.** The picker
would then name the cause instead of saying nothing. Rejected: it fixes the
message and not the wedge. The workspace is still down, and the only repair is
still a text editor outside the product.

**Widen `boot_failure::report` to cover every `main.rs` startup abort.**
Tempting, and unsafe. Reporting marks a failure terminal, which stops the
gateway respawning. Classifying a transient error that way converts a workspace
that would have recovered into a dead one. That module's doc comment warns
against exactly this. Making it safe needs a per-site terminal-or-transient
judgment, which is its own piece of work.

**Keep the whole-file load and only drop the abort.** Simpler, and it leaves
one bad entry able to disable every other proxy in the file at request time.
The user would meet the same fault one layer down.

**Answer 404 for a rejected provider, letting the builtin fill in.** Rejected on
the security ground above: it silently changes which backend the request
reaches.

**Have the migration delete or quarantine an entry it cannot translate.**
Rejected: the file is the user's, and a config the engine rewrote by deleting
part of it is worse than one it refused to serve. Leaving the entry in place
keeps the fix a one-line edit.
