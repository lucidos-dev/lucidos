---
hide:
  - toc
---

# Architecture

Six pictures, in the order the pieces stack up.

## One machine

Lucidos runs on your machine. A **gateway** answers the door, and behind it sits
one **engine** per workspace. Each engine owns its own event store and its own
files, so one workspace cannot read another's.

<div class="lx-svg" markdown="0">
{% include "diagrams/01-machine.svg" %}
</div>
<p class="lx-cap">The desktop app and a browser tab run on the machine. Your phone
reaches the same gateway over your network. Three kinds of traffic leave: model
calls, the outside APIs you set up in the proxy, and the commands your tools
run.</p>

## The event bus

Inside the engine, one bus carries everything that happens. The chat agent, the
coding agents, triggers and your own apps all emit onto it. Nothing writes
around it.

The bus stores the event **before** anyone hears about it. So a consumer never
reacts to something that is not yet on disk, and a restart picks up exactly
where the record ends.

<div class="lx-svg" markdown="0">
{% include "diagrams/02-event-bus.svg" %}
</div>
<p class="lx-cap">Producers on the left, consumers on the right, Postgres
underneath. Add a consumer and no producer changes.</p>

## One turn

A turn is a loop. The model answers or it calls a tool, the tool result goes
back to the model, and round it goes until the work is done.

<div class="lx-svg" markdown="0">
{% include "diagrams/03-a-turn.svg" %}
</div>
<p class="lx-cap">Every step is an event, so the turn is replayable and you can
read it back later.</p>

## Reaching an outside API

You describe an API once. After that the agent, your apps, your triggers and
your scripts all reach it the same way, through the **proxy**. The proxy adds
whatever that API wants: a key, a signature, a login handshake, or a signer you
wrote yourself. Your credentials stay in the engine.

<div class="lx-svg" markdown="0">
{% include "diagrams/04-proxy.svg" %}
</div>
<p class="lx-cap">Set it up once and everything in the workspace can use it.
The secret never reaches the chat, a log, or an app.</p>

## Hearing from an outside service

The proxy covers you calling out. A **webhook** covers the other direction: an
outside service telling you that something happened. You point GitHub, Stripe or
your own script at a URL, and each delivery becomes an event a trigger reacts to.

Those deliveries answer on their own port, the **hook socket**. It serves one
route and nothing else, which is what makes it the single door you can safely
publish to the internet. Everything else stays behind your network and your
paired devices.

<div class="lx-svg" markdown="0">
{% include "diagrams/05-webhook.svg" %}
</div>
<p class="lx-cap">Every delivery proves itself, by a token or by the sender's own
signature. A webhook fires the one event you pinned to it, whatever the sender
posts.</p>

## Staying in sync

One stream feeds every screen. Open the same workspace on a laptop and a phone
and both move together, because both read the same events.

<div class="lx-svg" markdown="0">
{% include "diagrams/06-stream.svg" %}
</div>
<p class="lx-cap">Close everything and the work still runs. You come back to the
new state in your apps and files, and it can notify you on the events you
choose.</p>

## Where to go next

- **[Concepts](concepts.md)**: what an event, an artifact, an app and a trigger are.
- **[Develop Lucidos](develop.md)**: the crates, and how Lucidos changes its own code.
