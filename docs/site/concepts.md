# Concepts

Everything you do is recorded as an **event**. Your files are **artifacts**. What
you want is an **intent**, and how to do it is **knowhow**. An **app** gives that
a UI, and a **trigger** runs it automatically. A **webhook** lets an outside
service set off a trigger.

## Events and artifacts

**Events** are the immutable record of what happened, and the source of truth.
**Artifacts** are your durable, git-tracked files.

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

Because events are the authority, these always hold:

{%
   include-markdown "../../README.md"
   start="<!--invariants-start-->"
   end="<!--invariants-end-->"
%}

## Intent, knowhow, and scripts

Lucidos keeps three things apart: **what** you want (stable, in your words),
**how** to do it (technical, evolving), and the **code** that does it.

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

An **app** is a UI you open repeatedly. A **trigger** runs work on a schedule or
in response to an event.

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

## Webhooks

A **webhook** receives events from an outside service, and a trigger can react to
them. It is the one surface a stranger can reach.

{%
   include-markdown "../../system-knowhow/glossary.md"
   start="<!--gloss-webhook-start-->"
   end="<!--gloss-webhook-end-->"
%}

## The prompt-first model

The prompt is the primary interface. You describe what you want, and Lucidos
builds the data and the presentation together, live.

{%
   include-markdown "../../system-knowhow/glossary.md"
   start="<!--gloss-live-cocreation-start-->"
   end="<!--gloss-live-cocreation-end-->"
%}

## Coming from other AI tools? The "skill" question

Other AI assistants often package "how to do X" as a **skill**. Lucidos splits a
skill into these pieces:

| Where a "skill" elsewhere bundles… | In Lucidos it's a… |
|---|---|
| Instructions the agent follows for a task | **Knowhow**: how-to docs the *Lucidos Agent* loads on demand |
| A reusable interface for the capability | **App**: a persistent UI under `data/apps/<id>/` |
| Helper code the instructions call | **Script** |
| The whole capability shipped as one installable | **Plugin**: a bundle of apps + knowhow + triggers + scripts |
| "Do this automatically / when X happens" | **Trigger** |

**Knowhow** is the closest match to a skill, but you never invoke it by name. The
agent matches your request against each knowhow file's description at runtime,
and loads the file when it is relevant. A new knowhow file is available as soon
as you add it.
