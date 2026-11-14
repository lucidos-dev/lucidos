# 0606: A parked card's keep-alive cap is learned per workspace from the cards asked near its local hour, frozen per wait, and every keep-alive status names its cap source; amends 0547 and 0603

- **Status**: Accepted
- **Date**: 2026-10-31
- **Amends**: [0547](0547-keep-alive-beats-a-parked-session-by-copy.md). A
  parked card no longer always keeps half the reread cap. Its worst case grows
  from 1.5 rereads per card to 2. Engages its rejected "Full break-even for
  parked waits".
- **Amends**: [0603](0603-keep-alive-follows-the-expected-wake.md). History now
  sets a parked card's cap, and only that. Engages its rejected "History as the
  rule".

## Context

A parked card, a question card or a permission prompt, kept its cache warm for
half the reread cap and then stopped. Only prices moved that cap. Yet when a
person answers depends on the person, and it changes through the day. A card
asked late in the evening often waits until morning.

The cache card also stated its estimates without their evidence. A reader could
not tell a give-up time set by an agent's estimate from one set by a fixed rule.

## Decision

**A parked card's cap is learned from the cards asked near its hour.** Take
this workspace's cards of the same agent and card type, asked within two hours
either side of the ask's local hour. Pick the ping count with the least summed
cost over them, at the run's own beat interval and prices.

- **Floor.** At least 30 cards across at least 7 distinct days. Below it, the
  card keeps half the reread cap.
- **Ceiling.** One full reread cap of pings.
- **A card's wait ends when the cache is next read**: at the first real
  request after the card closes. An answer resumes the turn at once. A
  dismissal ends at the user's next message, and an allowed permission card at
  the allow, since its tool runs first. With no real request the card is
  undecided for a day, then abandoned: its pings are paid and no reread is
  saved. Question and permission cards share the rule.
- **The hour** is local to the `timezone` preference, and UTC when unset. The
  cap source carries that zone's UTC offset at the ask, so the card shows the
  hours on the user's own clock.

**The cap is frozen per wait.** It resolves at the wait's first status, and
later ticks read it and its odds back from that status. An hourly history
rebuild never moves a running wait.

**A cap of c pings sends c pings.** The run counts its pings and stops after
the c-th. Its give-up time is an estimate: the last refresh, plus the beats
left, plus the beat window's width. With every ping out it is the last
refresh, so a ping that lands late never reopens a stopped run.

**History sets nothing else.** Event waits and working children keep the
expected-wake plan of ADR 0603.

**Every keep-alive status names its cap source**, a `cap_source` tagged by
`kind`. The card says it, for example "3 pings, learned from 41 waits like this
asked 22:00 to 03:00". The odds in "Show the math" count the same hour window
that set the cap.

## Rationale

The evidence is one heavy workspace at one price ratio, and it is weak. A
held-out test replayed the engine's own rules over 59 days of question cards:
a 30-day trailing fit, refit every five days, at each agent's real beat. The
score is the share of reread cost saved against no keep-alive:

| Reread price | Learned by hour minus half the cap | 95% CI |
|---|---|---|
| 25 pings | +1.0 points | -0.8 to +2.8 |
| 12.5 pings | +1.7 points | +0.2 to +3.4 |

- The plan's first measurement showed +5.3 points. Almost all of it came from
  counting a canceled card as answered when it was canceled. Most cancels have
  no real request after them, so that overstated the answers.
- Idle time at the ask added nothing. A person often acts, the agent asks a
  minute later, and the person has gone.
- At that workspace's spend, the whole keep-alive saves about 0.4% net of its
  pings, and the learned cap adds under 0.1%. Rewrites after a reply to a
  finished thread are about 18%, and the keep-alive does not reach them.

The cap ships anyway, with that evidence stated. Its expected gain is small
but positive, and it is the base a learned keep-alive for finished threads
would reuse.

ADR 0603 rejected history as the rule for event waits, because history says
nothing about a given build. That holds there. A person's answer is different:
no model can estimate it, and its hour is the strongest signal measured.

ADR 0547 rejected the full reread cap for parked waits, because long beats paid
for cards nobody answered. The learned cap reaches the full cap only where this
workspace's own cards at that hour say it pays.

The floor keeps a thin history from setting a cap, so every new install keeps
today's rule. The freeze keeps the card's give-up time honest for one wait.

## Consequences

- A workspace past the floor accepts a worst case of 2 rereads per card.
- New installs and most permission cards keep half the cap until their history
  passes the floor.
- On a long cache a learned cap can mean hours of pings, still within one
  reread.
- A user who travels without changing `timezone` gets caps for the wrong hour.
  The 30-day window relearns a lasting move.
- A stored status without `cap_source` still reads, and the card shows its old
  sentence.
- Each beat lands a little after its anchor, from the 10-second tick and the
  copy's start-up. So a learned run's give-up time moves later by that much
  with each ping. The count does not move.
- A closed card with no real request after it stays out of history for a day.

## Alternatives considered

- **Idle time at the ask as a second key.** It scored no better than the hour
  alone, and it costs a human-action definition. Deferred until a better
  presence signal exists.
- **One learned cap per agent, with no hour.** It matched half the cap within
  noise.
- **A fixed clock rule**, such as no pings at night. Every person's night is
  different, and the learned window finds it unaided.
- **Re-fit the cap on every tick.** A rebuild would move a running wait's
  give-up time, and a stopped card could turn warm again.
- **Day of week as a second key.** A weekday/weekend split scored -0.3 points
  against the hour alone (95% CI -1.3 to +0.6): fewer cards per window, no
  gain. A single weekday gets about 4 days in 30, below the floor.
- **A canceled card always abandoned.** Some cancels are dismissals followed by
  a typed message, which reads the cache. The next real request says which.
- **A give-up time of c beats after the ask.** Beats drift late, so a long cap
  sent fewer pings than its card said. A long asking request could add one.

Plan, measurements and invariants:
`docs/plans/2026-10-31-card-cap-learned-and-sourced.md`.
