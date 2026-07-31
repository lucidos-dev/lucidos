# Concepts

Lucidos is built from a small set of concepts that compose. Everything you do is
recorded as an **event**; your files are **artifacts**; what you want is an
**intent** and how to do it is **knowhow**; **apps** give that a UI and **triggers**
make it happen automatically. This page defines each one, straight from the
canonical glossary and taxonomy.

## Events and artifacts

The two ways Lucidos remembers: **events** are the immutable record of what happened
(the source of truth), and **artifacts** are your durable, git-tracked files.

{%
   include-markdown "../../system-knowhow/glossary.md"
   start="<!--gloss-event-start-->"
   end="<!--gloss-event-end-->"
%}

{%
   include-markdown "../../system-knowhow/glossary.md"
   start="<!--gloss-artifact-start-->"
   end="<!--gloss-artifact-end-->"
%}

Because events are the authority, a few things always hold:

{%
   include-markdown "../../README.md"
   start="<!--invariants-start-->"
   end="<!--invariants-end-->"
%}

## Intent, knowhow, and scripts

Lucidos separates **what** you want (stable, in your words) from **how** to do it
well (technical, evolving) from the **code** that does it.

{%
   include-markdown "../taxonomy.md"
   start="<!--concepts-content-types-start-->"
   end="<!--concepts-content-types-end-->"
   heading-offset=1
%}

{%
   include-markdown "../taxonomy.md"
   start="<!--concepts-intent-knowhow-start-->"
   end="<!--concepts-intent-knowhow-end-->"
   heading-offset=1
%}

## Apps and triggers

An **app** is a UI you open repeatedly; a **trigger** runs work on a schedule or in
response to an event.

{%
   include-markdown "../../system-knowhow/glossary.md"
   start="<!--gloss-app-start-->"
   end="<!--gloss-app-end-->"
%}

{%
   include-markdown "../../system-knowhow/glossary.md"
   start="<!--gloss-trigger-start-->"
   end="<!--gloss-trigger-end-->"
%}

## The prompt-first model

The prompt is the primary interface: you describe what you want, and the system
materializes it — data and presentation together, live, with no build-and-deploy gap.

{%
   include-markdown "../../system-knowhow/glossary.md"
   start="<!--gloss-live-cocreation-start-->"
   end="<!--gloss-live-cocreation-end-->"
%}

## Coming from other AI tools? The "skill" question

If you've used other AI assistants, you're probably looking for **skills** — the
common name for a packaged unit of "here's how to do X." Lucidos deliberately has no
single thing called a skill; it splits that idea into sharper, separately-useful
pieces:

| Where a "skill" elsewhere bundles… | In Lucidos it's a… |
|---|---|
| Instructions the agent follows for a task | **Knowhow** — how-to docs the *Lucidos Agent* loads on demand |
| A reusable interface for the capability | **App** — a persistent UI under `data/apps/<id>/` |
| Helper code the instructions call | **Script** |
| The whole capability shipped as one installable | **Plugin** — a bundle of apps + knowhow + triggers + scripts |
| "Do this automatically / when X happens" | **Trigger** |

The closest single analog to a skill is **knowhow** — but with one defining
difference: you don't invoke knowhow by name. The agent *discovers* the right
knowhow semantically at runtime, matching your request against each file's
description, and loads it when it's relevant. Adding a new recipe just works; there's
no command to register and no menu to wire up.

!!! note "Why you'll still see \"Skills\" in a few places"
    Some older material (release notes, design records) uses *Skills* for what is
    now split into *apps* and *knowhow*. The vocabulary moved on: the canonical
    terms are the ones in the table above.
