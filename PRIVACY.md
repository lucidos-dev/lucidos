# Privacy

Lucidos is **local-first**: the engine runs on your own machine, and your
workspace data lives on your filesystem and in a local PostgreSQL database that
you control. This document explains what is stored locally, and when data leaves
your machine and why. It also sets out, in full, the one recurring request
Lucidos makes on its own.

> **Pre-1.0.** Lucidos is pre-1.0 — the newest `v*` tag is the current
> version. Behaviour can change before 1.0; this document describes the
> current release. If a change affects what leaves your machine, we'll call it
> out in the [CHANGELOG](CHANGELOG.md).

## TL;DR

- Your workspace — events, threads, messages, memory, artifacts, settings — is
  stored **locally** (filesystem + local Postgres). Lucidos has no server, no
  account, and no cloud sync.
- **No analytics, no usage statistics, no crash reports.** Nothing about what
  you do in Lucidos is collected or sent anywhere.
- **One recurring request.** Once an hour Lucidos asks `lucidos.dev` whether a
  newer version is published. It sends your platform, your architecture and the
  version you run, and nothing else. It is set out in full
  [below](#update-checks), and you can turn it off.
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

### Update checks

Once an hour the **gateway** asks `lucidos.dev` whether a newer version of
Lucidos is published, so it can offer you the update. This is the only request
Lucidos makes on a schedule you did not configure. It is also the only one that
reaches a server the Lucidos project operates.

One request per machine, whatever you are running: the gateway is machine-global
and every open window reads its one answer. It covers every install, the macOS
app and the `curl … | sh` runtime alike, on macOS and on Linux.

**What it sends.** Three values, in the URL:

| Value | Example | Why |
|---|---|---|
| platform | `macos` | so the answer names a build that exists for you |
| architecture | `aarch64` | the same |
| version | `1.2.3` | so the origin can answer an old version correctly |

**What it also reveals.** Like any web request it carries your **IP address**,
which our CDN (Cloudflare) sees while terminating TLS. An hourly request from
one address therefore shows that Lucidos was running there. We say so plainly,
because "platform, architecture and version" alone would be a half-truth.

**What it does not send.** No account, no machine identifier, no workspace name
or count, no counter, and nothing about what you use Lucidos for. It carries no
cookie and no credentials, and the client follows no redirect.

**What we do with it.** We read aggregate request counts per platform, so the
project can tell roughly how many installs exist. We retain no per-request
identity for this route.

**Nothing installs itself.** The check only tells you a version exists. Taking
it is your click: the macOS app installs and relaunches, and a headless install
gives you the exact `install.sh` command to run.

**It is on by default, and you are not asked first.** Lucidos does not open with
a consent dialog about it, for the same reason `npm`, `cargo`, `gh` and Homebrew
do not. This page is the notice, and the switch named below is the control.

**Turning it off.** Settings > System > Overview > Check for updates automatically, or set
`enabled = false` under `[release_check]` in `~/.lucidos/updates.toml`. The
gateway re-reads that file on every tick, so it stops at once. The **Check for
Updates** button on that same page still works while it is off. Turning it off
therefore costs you nothing but the automatic poll.

**A dev build never checks.** A gateway launched from a source checkout makes no
request at all, whatever its configuration says.

### Release notes

Opening **Settings > System > What's New** fetches the project's published
changelog. That is what lets the panel show you a release newer than the copy
you are running. The request goes to
`raw.githubusercontent.com/lucidos-dev/lucidos/main/CHANGELOG.md` and tells
GitHub your IP address, under GitHub's privacy policy. It is a plain download of
a public file. It carries no workspace data, no usage information, and not even
your version.

Unlike the update check above, this is **not** recurring: it happens when you
open the panel, never on a schedule, and the answer is reused for hours. If it
fails, the panel silently shows the release notes that shipped inside your own
copy, so it still works offline.

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

Lucidos collects **no telemetry**. It gathers no analytics and no usage
statistics, and sends no crash or error report to us or to anyone else. It
records nothing about what you do with it. Lucidos originates no network traffic
beyond the activity described above:

- LLM, tool, coding-agent and plugin calls. Each one serves a task you
  initiated, or polls a source you configured.
- The gateway's hourly [update check](#update-checks) against `lucidos.dev`,
  which sends your platform, architecture and version and nothing else. You can
  turn it off.
- The *What's New* panel's download of the published changelog, when you open
  it.
- The service worker's checks against **your own local engine** for a fresh
  frontend build.

The update check is the one item there that reaches a server we operate. It is
worth being exact about that trade. Until it existed, your privacy here rested
on our **inability** to see anything: the check went to GitHub, whose logs we
cannot read. Now it rests on the design above, and on our not looking. That is a
real change in kind, and we would rather state it than have you find it.

## Questions and reports

For questions about this document, open a
[GitHub Discussion](https://github.com/lucidos-dev/lucidos/discussions). If you
believe you've found a privacy or security **vulnerability** (for example, a way
data leaks that this document says it shouldn't), please **don't** open a public
issue — follow the private disclosure process in [SECURITY.md](SECURITY.md).
