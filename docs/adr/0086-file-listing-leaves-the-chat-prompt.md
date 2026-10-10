# 0086: The file listing leaves the chat prompt: files are discovered on demand, and the freed budget goes to knowhow routing

- **Status**: Accepted
- **Date**: 2026-08-18
- **Amended**: 2026-08-18. The decision is now conditional, and three of the
  claims supporting it are corrected. Read the amendment at the end before
  acting on the Decision below.

## Context

The *workspace payload* (`engine/chat/process/workspace_payload.rs`) is three
sections in one budget: the `[CURRENT FILES]` listing, the Available Apps list,
and the knowhow routing list. `WORKSPACE_PAYLOAD_BUDGET_CHARS = 12_800` caps all
three together, enforced by `busy_workspace_payload_stays_under_budget`.

The listing is already a sample rather than an inventory: `MAX_DIRS = 40`,
`MAX_FILES_PER_DIR = 8`, `MAX_FILES_TOTAL = 120`. The comment on the
per-directory cap says eight files is "what KIND of directory it is, which is
all a routing signal needs". Its one job is resolving a phrase like "the report"
to a path without a tool call.

The three sections look alike and are not. They differ in whether the agent has
another way to get the same thing.

## Decision

**Remove the `[CURRENT FILES]` listing from the chat prompt.** Files are
discovered on demand through `list_files`, `glob_files` and `grep_files`.

The Available Apps list and the knowhow routing list stay. The budget constant
stays where it is, so the headroom the listing used goes to those two, and in
practice to routing.

This is unconditional. It is not gated on ADR 0085's experimental context mode,
and applies whether that flag is on or off.

## Rationale

**The three sections have three different discovery stories, and only one of
them is load-bearing.**

- **Knowhow routing cannot go.** `load_knowhow` takes an id and nothing lists
  them, so the routing list is the only way the agent learns a recipe exists.
  The descriptions are also the retrieval index, since matching happens against
  that text.
- **Apps sit in the middle.** `list_apps` exists, so discovery is possible. But
  every named app must be linked as `[Habit Tracker](app:habit-tracker)`.
  Dropping the list would put a tool call in front of almost any reply that
  mentions one.
- **Files have three replacements**, and all three walk everything rather than a
  truncated sample.

**The sample fails exactly where it would be needed.** 120 files out of a large
workspace is a lottery ticket for path resolution. The agent falls back to
`glob_files` anyway and has paid for the listing twice. On a small workspace the
120 might be everything, and there the tool call was cheap to begin with. The
value runs inversely to workspace size.

**Removing it buys routing quality, not just tokens.** One budget covers all
three sections, so the listing was competing for space with the only section
that has no alternative. This is a reallocation more than a cut.

**It is also a listing of a moving target.** Files change constantly, including
while a turn is running. So it is stale by construction in a way an app
inventory and a knowhow set are not.

## Consequences

- **The agent pays a tool call when it needs a path it cannot guess.** That is
  the intended trade, and `glob_files` answers better than the sample did.
- **The knowhow routing list gets the headroom.** Whether that becomes more docs
  or longer descriptions is a separate call, bounded by
  `KNOWHOW_DESCRIPTION_MAX_CHARS = 400`.
- **`busy_workspace_payload_stays_under_budget` needs updating**, since it sums
  three sections and will sum two. Keep the constant: the space is reallocated,
  not reclaimed.
- **The `File List` row leaves the LLM Context Viewer**, since
  `build_capture_sections` will no longer be handed one.
- **A small workspace loses a real convenience.** Where the listing was complete
  it saved a round trip on the common "open the report" request.
- **It slightly weakens ADR 0085's control arm.** An eval comparing lean against
  today compares against a today that just changed, so the two should not land
  in the same measurement window.

## Alternatives considered

**Keep it as it is.** Rejected. It costs a share of a budget the routing list
needs more. It is also the one section of the three with a full set of
replacements.

**Shrink it further instead of removing it.** Rejected. It is already at 40
directories and 8 files each, and shrinking a sample makes the lottery worse
rather than cheaper. There is no size at which a sample of a large tree resolves
a specific path reliably.

**Send it on the thread's first call only.** Rejected. That is ADR 0085's shape
applied to the wrong section. Files change constantly, so a first-call listing
is stale within minutes and the agent has no signal that it went stale. A tool
call is correct at the moment it is made.

**Replace it with a directory-only map, no filenames.** Rejected for now, and
the closest thing to a survivor. It would keep the shape signal at a fraction of
the size. It also duplicates the workspace taxonomy, which the system prompt and
knowhow already state, so it is likely to be paying twice for one fact.

## Amendment, 2026-08-18: an inventory or nothing, never a sample

A measurement landed after this was accepted, and before the engine change was
made. Everything below rests on it:
`artifacts/file-listing-replacement-investigation.md` in the `dev` workspace,
which carries the full evidence, the cost ledger and the reproduction notes.
Nothing is being reverted, because the removal never shipped.

**The core decision holds. "This is unconditional" does not.** The block earns
its cost when it is a complete inventory and does not when it is a sample. The
variable was never the idea. It is whether the block names everything.

### The corrected decision

**The chat prompt carries a file listing only when it names every non-vendored
file. A sample is never sent.** Where the Decision above says "Remove the
`[CURRENT FILES]` listing from the chat prompt", read: remove the sample. The
sentence "This is unconditional" is retracted.

Past the ceiling the block disappears entirely, which is the case this ADR is
about. There is no header, no partial listing and no "and N more". The agent
gets an inventory or nothing.

### The evidence, in one table

The same block, on the same workspace, a quarter apart. In May the workspace
held 54 to 82 files and the listing was complete. Today it is a 9.8% sample.

| First touch of an in-scope file | May/June, complete | August, sampled |
|---|---:|---:|
| Turns measured | 74 | 84 |
| Path present in the listing | 75.7% | 19.0% |
| Listing plausibly supplied the path | 50.0% | 2.4% |
| Novel path reached with no discovery call | 95% | 37% |

The last row is the mechanism. With a complete listing the agent almost never
paid for discovery, and now it pays most of the time. Today's whole observed
contribution is 5 paths across 4 turns in ten days. That is about 14x
underwater, against a break-even of 5.6 saved round trips per day.

"Plausibly supplied" is an upper bound: the path was named, was not already in
the thread, and was reached with no discovery call. It is not proof of cause.
The control arm is underpowered and rules nothing out either way, with a 95%
interval from -32 to +24 points.

### Three claims above are wrong, and this is the correction

**"On a small workspace the 120 might be everything, and there the tool call was
cheap to begin with" is backwards.** At 16 files the listing is 614 chars and
pays for itself once every 28 turns. The tool call it saves is roughly 27x its
price. Small workspaces are the population this block serves best, not a case
where it does not matter.

**"Removing it buys routing quality, not just tokens" is false.** So is the
title's "the freed budget goes to knowhow routing". The three sections are not
in one budget. Apps and knowhow are appended to the system prompt. The file list
rides in the per-turn user message, and the two are billed in different cache
tiers.

Per 1,000 chars per turn a knowhow char currently costs more than a file-list
char. The classifier suppresses the file list on 43.6% of turns and never
suppresses the routing list. `WORKSPACE_PAYLOAD_BUDGET_CHARS` is a ratchet over
a synthetic fixture, so what it frees is fixture headroom and not wire budget.
Removing the listing frees nothing for routing. It only costs less.

Longer knowhow descriptions may still be worth buying, at $0.66 per day for the
uncapped set. That is a new spend, argued on its own merits.

**The directory-only map is rejected, not rejected for now.** Two independent
cuts settle it. 87.0% of `glob_files` patterns and 77.8% of `grep_files` path
scopes already name a directory at depth 2 or more. 56.9% of those anchors are
app ids the Available Apps list already supplies.

That counts only the searches the agent wrote, so the addendum tests the ones
that missed. Of 629 discovery calls in 30 days, only 6 (1.0%) named a directory
that did not exist. The cheapest honest map is depth 2 at 2,969 chars, costing
$0.43 per day to save $0.020 per day. It is 22x underwater, and it mostly
restates the `WORKSPACE LAYOUT` block the system prompt already carries.

### The re-derived ceiling

`MAX_DIRS = 40` is what broke completeness first. On `dev` the listing stopped
being complete at 54 files, because 73 files were spread over 52 directories.
That is inside the range where the block demonstrably worked, so leaving the cap
would switch the block off exactly where it pays.

All three caps are gone. Once the rule is all-or-nothing they shape nothing.
Their only remaining job is deciding whether a workspace is small enough to be
listed whole, and three numbers cannot express one threshold. Worse,
`MAX_FILES_PER_DIR = 8` would have become the binding one, killing the block at
nine files in a single directory.

They are replaced by one ceiling on the rendered block,
`FILE_LIST_MAX_BYTES = 4_000`. Two independent derivations land within 2% of
each other.

**Coverage.** Value was demonstrated at 54 to 82 files, and a complete listing
costs 31 to 47 chars per file. At the densest observed rendering, 82 files is
3,854 chars. A lower ceiling cuts into the range the evidence covers.

**Break-even.** The ledger gives 2.538 chars per token, one write at $6.25/MTok,
17.06 later reads at $0.50/MTok, and $0.0976 for a saved round trip. A block of
C chars costs about `C x 5.8e-6` per turn that carries it. To pay, it must save
a round trip on about `C x 6.0e-5` of them.

The best estimate of a complete listing's supply rate is 23.3%. That is the
50.0% above, times the 46.5% of listing-carrying turns that touch a file.
Break-even at that rate is 3,906 chars.

4,000 is the round number just above both anchors. The investigation suggests
"around 5,000", which would need a 29.8% supply rate and is past anything
measured. Measuring the rendered block rather than counting files is the part
that matters most: a later reader who wants a different number should move it on
the cost, not reintroduce a directory count.

### Consequences, revised

- **A small workspace keeps the convenience.** The original consequence "a small
  workspace loses a real convenience" no longer applies. That population is
  exactly the one the amendment protects.
- **The block self-retires.** A workspace that grows past the ceiling loses it
  and can never regain a misleading version.
- **Flapping at the boundary is free.** The block lives in the volatile user
  message, which is rebuilt every turn regardless, so a workspace hovering at
  the ceiling invalidates no cache.
- **`busy_workspace_payload_stays_under_budget` still sums three sections.** The
  original consequence said it would sum two. Its fixture is 90 files and now
  renders whole, so the file-list area grows rather than disappearing.
- **The `File List` row leaves the LLM Context Viewer only past the ceiling.**
  `build_capture_sections` filters empty bodies, so the row tracks the block.
- **Follow-up, not fixed here: two cross-references are now stale.** ADR 0085
  rejects "dropping the file listing at round 2" because 0086 "removes it
  altogether" and is "unconditional rather than tied to this flag". ADR 0087
  says 0086 "removes the file listing unconditionally and says so itself". Both
  records are owned elsewhere, and the reasoning each supports survives: 0087's
  eval still cannot span this change. Only the word is wrong.
- **The index line for this ADR still carries the retracted clause.**
  `docs/adr/index.md` is `merge=union`, so rewording a line a sibling branch
  still holds produces two entries and a `check-adrs.sh` failure.

### What this amendment does not change

**"Shrink it further" stays rejected, and is strengthened.** There is no size at
which a sample of a large tree resolves a specific path reliably, and the answer
was never a smaller sample.

**"Send it on the thread's first call only" stays rejected**, on its original
reasoning. So does "keep it as it is", though its budget argument is void.

**The block's own text is unchanged.** The 50% regime is this exact block.
Rewording it in the same change would put an unmeasured intervention inside the
measured one.

The measurement could not establish three things, and they bound all of the
above. There is no counterfactual, since the control arm is underpowered. Turns
that never touch a file are unmeasured, so 3.3% is a floor. And no eval exists,
so every number here is tokens and tool calls rather than answer quality.
