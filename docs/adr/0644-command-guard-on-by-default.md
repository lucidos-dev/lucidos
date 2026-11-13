# 0644: The command guard defaults on; supersedes ADR 0002's ships-dark default

- **Status**: Accepted
- **Date**: 2026-11-10

## Context

ADR 0002 shipped the command guard dark: `command_guard` defaulted to `false`,
and only the Settings toggle wrote a row. So every new workspace and fresh
install started unguarded. With the guard off, a Lucidos Agent thread runs any
bash or python above the catastrophic floor and the secret-read block.

That matters most for triggers. Hostile input to a trigger (mail, chat, web
pages, issues) can steer its agent to do anything. ADR 0641's coding-agent spawn
gate buys little while the trigger's own lane is unguarded. No ADR argued for
off as the product default.

Two things made a plain flip unsafe. Coding-agent answering nested under the
master switch (`command_guard_coding_agents`, default `true` under the guard).
So turning safety on would also have let the judge answer coding-agent cards.
And with no recommended model served, the judge fell back to the chat model.
That ran a security decision on an unmeasured model in silence.

Plan: `docs/plans/2026-11-10-command-guard-on-by-default.md`.

## Decision

The command guard is on by default for every Lucidos Agent lane: chat, trigger,
and MCP launch approval. A workspace with no `command_guard` row runs guarded.
Only an explicit `false` turns it off. Four rules come with the flip:

1. **Coding-agent auto-answer splits off and defaults off.** It is its own
   setting, `coding_agent_auto_answer`, and never reads `command_guard`. It
   shares only the classifier and the judge. Only auto-answer lets the judge
   answer a coding agent, a Codex sandbox escape included. A migration wrote
   each workspace's effective value: the guard `true` AND the old
   `command_guard_coding_agents` row, which defaulted `true`.
2. **Only the owner widens a side-effect grant**, as ADR 0641 made the
   coding-agent grant. An agent or app frame gets a refusal naming the owner.
   Anyone may narrow a grant or re-send a held one. A plugin `trigger.toml`
   installs with an empty grant. An unowned edit to a granted trigger's `run`,
   `on` or `mcp_servers` clears both grants.
3. **The judge never runs silently on the chat model.** Its model resolves only
   to a measured default a configured provider serves (ADR 0406 addendum), or
   to the owner's pick. A picked System One backend also resolves it, and then
   runs alone. With the guard and the judge on and nothing resolved, a
   Lucidos Agent turn refuses to start and names Settings > Models. Coding-agent
   auto-answer cards instead. Turning the judge off is an explicit choice: the
   static lists decide.
4. **A providerless workspace keeps its own error.** With no LLM provider
   configured, the turn fails with "No LLM provider configured", not the judge
   refusal. The judge is not the fault there, and naming it would send the owner
   to the wrong fix.

Existing workspaces with no row flip on. A release notice,
`command-guard-on-by-default`, reaches only those the flip affects: no row, plus
a trigger in the registry, an MCP server with no launch approval, or an
unresolved judge model. It links each choice the flip leaves the owner: the
guard, the judge model and coding-agent auto-answer. Its action runs only the
matching workspace audit checks.

## Rationale

- **Safe by default is the product's promise.** ADR 0002 already made the guard
  selective: the safe 95% runs with no card. Data from a workspace guarded for
  30 days: 1 chat permission card and 26 command checkpoints, against 2,180
  judge calls, most from coding-agent answering. The cost of on falls on the
  judge, not on the user.
- **Turning safety on must never mean asking less.** Under the old nesting, the
  flip would have handed coding-agent cards to the judge in every workspace that
  never chose it. Splitting the setting keeps each switch meaning one thing.
- **A guard that agents can widen is not a guard.** With the guard on, the
  side-effect grant is what lets a trigger act unattended. If the trigger's own
  agent could widen it, hostile input could too.
- **A silent fallback hides a weaker guard.** The chat model was never measured
  on the judge eval. Refusing the turn is loud, and the refusal names the fix.

## Consequences

- A trigger that performs a side effect outside its grant now fails its run, and
  the failure notification says why. The owner grants the category once.
- An MCP server with no launch approval no longer starts unattended. The owner
  approves its launch line once.
- Some installs serve none of the judge defaults: local-only, xAI-only, OpenCode
  Free. Their Lucidos Agent turns refuse until the owner picks a judge model or
  turns the judge off. The release notice warns them first.
- There is no audit check for an unresolved judge: that state refuses the
  audit's own turn, and the refusal names the fix. The notice links the
  Command judge row instead.
- Unforgeable caller identity stays out of scope. Until it lands, a local
  process that drops its token still reads as the owner, for both grants.

## Alternatives considered

| Option | Why it lost |
|---|---|
| Keep it dark, and promote the toggle in onboarding | Every existing workspace and every skipped prompt stays unguarded. Triggers are exposed exactly where nobody watches. |
| A per-lane default (chat off, trigger on) | Two preferences for one threat. Chat is the lane that already asks, so on costs it least. |
| Flip, and keep coding-agent answering nested | Turning safety on would answer coding-agent cards for the user, which loosens the coding-agent lane. |
| Let the guard's switches keep judging Codex sandbox escapes | Default-on would auto-allow judge-placed escapes in every workspace that never stored a choice. |
| Copy stored auto-answer rows unchanged | A stored `true` under an off guard did nothing. Copying it would turn auto-answer on for a workspace that never ran it. |
| Seed side-effect grants or launch approvals on upgrade | Every seed would come from what the agent did before, which is exactly what the guard distrusts. The notice and the audit put the choice in front of the owner, as ADR 0641 did. |
| Fall back to the chat model, or run static-only, when no judge model resolves | Both run a weaker guard in silence. The owner would not learn that the guard changed. |
