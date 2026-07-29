---
name: Knowhow contribution
about: Propose a knowhow doc, app, or trigger to contribute
title: ""
labels: knowhow
assignees: ""
---

<!--
Lucidos is more than an engine — it's the apps, triggers, and knowhow that make a
workspace useful. This template is for proposing one of those.

Before filing, please skim:
  - docs/taxonomy.md                    — where things live and the survivability test
  - system-knowhow/building-knowhow.md  — how to write knowhow
  - system-knowhow/building-an-app.md   — how to build an app
  - system-knowhow/building-a-trigger.md — how to build a trigger
  - the glossaries (system-knowhow/glossary.md, docs/glossary.md) for the terms below
-->

## What are you contributing?

<!-- Pick one. -->

- [ ] **Knowhow** — a doc capturing *how* to achieve something (evolves over time)
- [ ] **App** — a UI + behavior the user can describe and run
- [ ] **Trigger** — scheduled or event-driven automation
- [ ] Other (explain below)

## What user intent does it serve?

What does the user want to accomplish? State the **intent** (the stable goal),
separately from the **knowhow** (the how, which can change). For a **trigger**,
phrase `run.intent` as what the user would say ("notify me when X happens"), and
keep the procedure (how to check, parse, retry) in knowhow — not in the intent.

## Summary

A short description of the knowhow / app / trigger and what it does.

## Where it lives

<!-- Apply the survivability test from docs/taxonomy.md: "Does this survive if I
     delete the app?" -> top-level (data/knowhow, data/triggers). "Only makes
     sense for this app?" -> inside the app (data/apps/<id>/). -->

- **Proposed location:** <!-- e.g. data/knowhow/<name>.md, data/apps/<id>/, data/triggers/<name>/ -->
- **Name:** <!-- describe by what it does — never generic names like app.md / knowhow.md -->

## Dependencies

<!-- Does it rely on external APIs, credentials, a plugin, or specific events?
     Does it need an auth handshake (see building-an-auth-handshake.md)? -->

## Anything else

<!-- Examples, prior art, related apps/triggers, open questions. -->
