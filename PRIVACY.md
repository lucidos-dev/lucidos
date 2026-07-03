# Privacy

Lucidos is **local-first**: the engine runs on your own machine, and your
workspace data lives on your filesystem and in a local PostgreSQL database that
you control. This document explains what is stored locally, when data leaves
your machine and why, and — importantly — that Lucidos collects **no telemetry**.

> **Pre-1.0.** Lucidos is currently on the **0.9.x** line. Behaviour can change
> before 1.0; this document describes the current release. If a change affects
> what leaves your machine, we'll call it out in the [CHANGELOG](CHANGELOG.md).

## TL;DR

- Your workspace — events, threads, messages, memory, artifacts, settings — is
  stored **locally** (filesystem + local Postgres). Lucidos has no server, no
  account, and no cloud sync.
- **No telemetry.** Lucidos does not collect analytics, usage statistics, or
  crash reports, and does not phone home.
- Data leaves your machine **only** when you (or something you set up) invoke a
  feature that talks to a third party — most importantly an **LLM call**, where
  your prompt and its context are sent to the provider you configured.
- Credentials and tokens are stored locally and are sent **only** to the
  specific third-party API each one belongs to.

## What is stored locally

Everything in a workspace lives under your control, in two places (see the
[README](README.md#workspace-structure) for the on-disk layout):

- **A local PostgreSQL database** (event store, with `pgvector` for memory) —
  the append-only event log is the source of truth. This includes your
  conversation history, thread metadata, notifications, memory embeddings,
  preferences, and the registries below.
- **Git-tracked files under `data/`** — your artifacts, apps, triggers, and
  knowhow.

Embeddings for memory are computed **in-process by a local model** (fastembed);
the text you store is not sent to a third party to be embedded.

Nothing in this section is uploaded anywhere by Lucidos. Backups, exports, and
moving a workspace between machines are actions **you** take explicitly.

## Credentials and tokens

API keys, OAuth tokens, SMTP/email logins, and other secrets you add are stored
locally in your workspace database (the `credentials`, `oauth_accounts`, and
`email_accounts` tables). They are:

- **Kept on your machine.** A secret never leaves your machine except inside the
  requests Lucidos makes to the **specific third-party API that credential is
  for** — e.g. a GitHub token is sent to GitHub, an LLM key to that LLM
  provider, an SMTP password to your mail server.
- **Never broadcast in events.** Credential-related events record only the
  service name, never the secret value, so secrets don't travel over the
  internal event/SSE stream to connected browser tabs.

You are responsible for the third-party accounts you connect and for the terms
that govern them.

## When your data leaves your machine

### LLM calls

Lucidos is built around a large language model, and **invoking it sends data off
your machine.** When the agent runs — when you chat, when a trigger fires, when
an app or coding-agent thread calls the model — your **prompt and its context**
are sent to the LLM provider you have configured. That context can include the
conversation, retrieved memory, and the content of files or artifacts relevant
to the request.

You choose the provider via configuration (`LUCIDOS_MODEL` and the model
registry). Supported backends are **Anthropic**, **Google Vertex AI**, and
**OpenAI**. Whatever is sent is handled under **that provider's terms and
privacy policy** — review the policy of the provider you use. Lucidos does not
add a layer of its own in between; it calls the provider you pointed it at,
using your credentials.

### Tools that make outbound calls on your behalf

Some built-in tools reach the network when the agent uses them. Each is a
deliberate call made to perform the task you asked for, not background
collection:

- **`web_search`** — queries a web search service.
- **`fetch_news`** — queries the public GDELT news API (`api.gdeltproject.org`).
- **Browser tool** — navigates to and loads the web pages you direct it to.
- **Email** — sends messages through the SMTP account you configured. (Sent-mail
  events record envelope metadata only — recipients, subject — never the message
  body.)
- **HTTP / API calls** — apps, triggers, and the proxy can call external APIs
  you set up, authenticated with the credentials you stored.

In addition, local models and assets (for example the embedding model) may be
**downloaded once** from their source on first use; after that they run locally.

### Coding-agent threads

When you run a coding-agent thread, Lucidos drives an **external coding-agent
CLI** as a subprocess — **Claude Code** (`claude`) or **Codex** (`codex`),
whichever you invoke. That tool is a separate program with its own network
behaviour:

- It sends the **code and context it works on** to **its own** model provider
  (Anthropic for Claude Code, OpenAI for Codex), under that tool's and that
  provider's terms. This is independent of the LLM provider you configured for
  the Lucidos agent.
- It runs ordinary developer commands **on your behalf** — `git` operations
  (clone / fetch / push to the remotes *you* configured) and dependency
  installs (`npm`, `cargo`, …) that reach the package registries those tools
  use.

### Plugins and plugin marketplaces

Plugins live in **git repositories**. Installing a plugin clones or fetches from
the repository its `source` points to; adding a plugin **marketplace** registers
a git repository that Lucidos can list plugins from. Once a marketplace is
registered, Lucidos polls it on a periodic background check and **auto-updates**
installed plugins from their source when a newer version is published.

**No marketplace is configured by default** — the marketplace registry is empty
until you add one, so there is no plugin-related network traffic until you
install a plugin or add a marketplace yourself.

## Telemetry: there is none

Lucidos contains **no telemetry**. It does not collect analytics or usage
statistics, does not send crash or error reports to us or any third party, and
does not phone home — there is no Lucidos-operated server for it to report to.
The only network traffic Lucidos originates is the activity described above —
LLM, tool, coding-agent, and plugin calls, each either serving a task you
initiated or polling a source you configured — plus the service worker's checks
against **your own local engine** for a fresh frontend build.

## Questions and reports

For questions about this document, open a
[GitHub Discussion](https://github.com/lucidos-dev/lucidos/discussions). If you
believe you've found a privacy or security **vulnerability** (for example, a way
data leaks that this document says it shouldn't), please **don't** open a public
issue — follow the private disclosure process in [SECURITY.md](SECURITY.md).
