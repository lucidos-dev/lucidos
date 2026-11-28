# 0067: The auto-detected browser-login domain list is never spliced into a prompt

- **Status**: Accepted
- **Date**: 2026-08-13

## Context

The `browser_logins` table is populated by `runtime::browser::session` from
whatever the user's persistent browser profile happens to hold a session for.
Nobody curates it. It is unfiltered browsing data, so it routinely names
employer sites, banking portals and anything else the user stays signed in to.

The chat system prompt once carried that list as a section. The theory was that
knowing where the user is already signed in would help the agent choose
`visible=true`.

## Decision

No prompt section lists auto-detected browser-login domains. The table and both
browser tools stay; only the prompt splice is gone.

## Rationale

The section shipped the user's browsing profile to the model provider on every
turn of every thread, and nothing consumed it. BROWSER TOOLS already tells the
agent to retry with `visible=true` on a login redirect, and
`browser_forget_login` takes an explicit domain from the user. So the whole
cost was paid for a capability two other surfaces already delivered.

`.claude/rules/no-private-data.md` sets the boundary this crosses: data the
user never chose to share is not example material, and a per-turn splice is the
widest possible exposure of it.

## Consequences

The agent does not know in advance which sites have a live session. It finds
out the way a person would, by trying and reacting to the login redirect, which
is what the prompt tells it to do.

Any future feature that wants this signal has to make the sharing explicit and
per-request, never resident. Re-adding a resident list reopens the same leak.

## Alternatives considered

**Splice a filtered list.** A denylist of "sensitive" domains cannot be written:
the sensitivity is the user's, not the domain's, and a personal email host and
an employer's SSO look identical to a filter.

**Splice on demand, when the agent asks.** Better than resident, but still
unnecessary: the redirect-then-retry path answers the same question at the
moment it matters, with no list to leak.

**Keep the list, drop the table.** Wrong layer. The table backs
`browser_forget_login`, which the user drives explicitly; the leak was the
prompt splice alone.
