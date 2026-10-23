---
hide:
  - toc
---

# Architecture

Six diagrams, in the order the pieces stack up.

## One machine

Lucidos runs on your machine. A **gateway** receives every request and passes it
to one **engine** per workspace. Each engine has its own event store and its own
files, so one workspace cannot read another's.

<div class="lx-svg" markdown="0">
{% include "diagrams/01-machine.svg" %}
</div>
<p class="lx-cap">The desktop app and a browser tab run on the machine. Your phone
reaches the same gateway over your network. Three kinds of traffic leave: model
calls, the outside APIs you set up in the proxy, and the commands your tools
run.</p>

## The event bus

Inside the engine, one bus carries everything that happens. The Lucidos Agent,
the coding agents, triggers and your own apps all emit onto it.

The bus stores each event **before** it notifies consumers. A consumer only sees
events that are already on disk, and a restart resumes where the record ends.

<div class="lx-svg" markdown="0">
{% include "diagrams/02-event-bus.svg" %}
</div>
<p class="lx-cap">Producers on the left, consumers on the right, Postgres
underneath.</p>

## One turn

A turn is a loop. The model either answers or calls a tool. The tool result goes
back to the model, and the loop repeats until the work is done.

<div class="lx-svg" markdown="0">
{% include "diagrams/03-a-turn.svg" %}
</div>
<p class="lx-cap">Every step is an event, so you can replay the turn and read it
back later.</p>

## Reaching an outside API

You describe an API once. After that the agent, your apps, your triggers and
your scripts all reach it the same way, through the **proxy**. The proxy adds
whatever that API wants: a key, a signature, a login handshake, or a signer you
wrote yourself. Your credentials stay in the engine.

<div class="lx-svg" markdown="0">
{% include "diagrams/04-proxy.svg" %}
</div>
<p class="lx-cap">The secret never reaches the chat, a log, or an app.</p>

## Hearing from an outside service

A **webhook** handles the other direction: an outside service tells you that
something happened. You point GitHub, Stripe or your own script at a URL, and
each delivery becomes an event that a trigger reacts to.

Deliveries arrive on their own port, the **hook socket**. It serves one route
only, so it is the one port you can safely publish to the internet. The rest of
Lucidos stays on your network and your paired devices.

<div class="lx-svg" markdown="0">
{% include "diagrams/05-webhook.svg" %}
</div>
<p class="lx-cap">Every delivery authenticates with a token or the sender's own
signature. A webhook fires the one event you pinned to it, whatever the sender
posts.</p>

## Staying in sync

One event stream feeds every screen. A laptop and a phone on the same workspace
stay in sync, because both read the same events.

<div class="lx-svg" markdown="0">
{% include "diagrams/06-stream.svg" %}
</div>
<p class="lx-cap">Work keeps running with every screen closed. When you return,
your apps and files show the new state. Lucidos can also notify you on the
events you choose.</p>

## Where to go next

- **[Concepts](concepts.md)**: what an event, an artifact, an app and a trigger are.
- **[Develop Lucidos](develop.md)**: the crates, and how Lucidos changes its own code.
