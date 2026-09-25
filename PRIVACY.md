# Privacy

Lucidos is **local-first**. The engine runs on your machine. Your workspace data
lives on your filesystem and in a local PostgreSQL database that you control.
This document lists what Lucidos stores locally and when data leaves your
machine.

> **Pre-1.0.** The newest `v*` tag is the current version, and this document
> describes it. Behaviour can change before 1.0. We note any change to what
> leaves your machine in the [CHANGELOG](CHANGELOG.md).

## TL;DR

- Your workspace (events, threads, messages, memory, artifacts, settings) is
  stored **locally**, on the filesystem and in local Postgres. There is no
  account and no cloud sync.
- Lucidos collects **no telemetry**: no analytics, usage statistics, or crash
  reports.
- **One recurring request.** Once an hour Lucidos asks `lucidos.dev` whether a
  newer version is published. It sends only your platform, architecture and
  version. [Update checks](#update-checks) explains how to turn it off.
- Other data leaves your machine when you, or something you set up, use a
  feature that talks to a third party. The main case is an **LLM call**: your
  prompt and its context go to the provider you configured.
- Credentials are stored locally. Each one goes only to the third-party API it
  belongs to.

## What is stored locally

A workspace lives in two places (see the [README](README.md#workspace-structure)
for the on-disk layout):

- **A local PostgreSQL database** (event store, with `pgvector` for memory). Its
  append-only event log is the source of truth. It holds your conversation
  history, thread metadata, notifications, memory embeddings, preferences, and
  the registries below.
- **Git-tracked files under `data/`**: your artifacts, apps, triggers, and
  knowhow.

A local model (fastembed) computes memory embeddings in-process, on your
machine.

You make backups, exports, and moves to another machine yourself.

## Credentials and tokens

API keys, OAuth tokens, SMTP/email logins, and other secrets you add are stored
in your workspace database (the `credentials`, `oauth_accounts`, and
`email_accounts` tables).

- **Each secret goes only to its own API.** Lucidos sends a secret only in
  requests to the third-party API it belongs to. A GitHub token goes to GitHub,
  an LLM key to that LLM provider, and an SMTP password to your mail server.
- **Events carry the service name only.** Credential-related events record the
  service name and omit the secret value. Secrets therefore stay off the
  internal event/SSE stream that reaches connected browser tabs.

You are responsible for the third-party accounts you connect and for the terms
that govern them.

## When your data leaves your machine

### LLM calls

Each time the agent runs, it sends your **prompt and its context** to the LLM
provider you configured. The agent runs when you chat, when a trigger fires, and
when an app or coding-agent thread calls the model. The context can include the
conversation, retrieved memory, and the content of relevant files or artifacts.

You choose the provider in configuration (`LUCIDOS_MODEL` and the model
registry). Supported backends are **Anthropic**, **Google Vertex AI**, and
**OpenAI**. Lucidos calls that provider directly, with your credentials. The
provider's terms and privacy policy govern what it receives.

### Tools that make outbound calls on your behalf

Some built-in tools reach the network when the agent uses them for a task you
asked for:

- **`web_search`** queries a web search service.
- **`fetch_news`** queries the public GDELT news API (`api.gdeltproject.org`).
- **Browser tool** loads the web pages you direct it to.
- **Email** sends messages through the SMTP account you configured. Sent-mail
  events record only envelope metadata: recipients and subject.
- **HTTP / API calls**: apps, triggers, and the proxy can call external APIs you
  set up, with the credentials you stored.

Some local models and assets, such as the embedding model, download once from
their source on first use. After that they run locally.

### Coding-agent threads

A coding-agent thread runs an **external coding-agent CLI** as a subprocess:
**Claude Code** (`claude`) or **Codex** (`codex`). That CLI is a separate
program with its own network behaviour:

- It sends the **code and context it works on** to **its own** model provider
  (Anthropic for Claude Code, OpenAI for Codex). That tool's and that
  provider's terms apply. This provider is separate from the one you configured
  for the Lucidos agent.
- It runs ordinary developer commands **on your behalf**. These include `git`
  operations (clone, fetch, push to the remotes *you* configured) and dependency
  installs (`npm`, `cargo`, …) that reach those tools' package registries.

### Update checks

Once an hour, the **gateway** asks `lucidos.dev` whether a newer version of
Lucidos exists. The gateway is machine-global, so each machine sends one request
per hour. This applies to the macOS app and the `curl … | sh` runtime, on macOS
and Linux.

**What it sends.** Three values, in the URL:

| Value | Example | Why |
|---|---|---|
| platform | `macos` | so the answer names a build that exists for you |
| architecture | `aarch64` | the same |
| version | `1.2.3` | so the origin can answer an old version correctly |

The request also carries your **IP address**, which our CDN (Cloudflare) sees.
An hourly request from one address shows when Lucidos was running there. We use
aggregate request counts per platform to estimate how many installs exist.

**Installing.** The check only reports that an update exists. In the macOS app,
you click to install and relaunch. A headless install shows the `install.sh`
command to run.

**Turning it off.** The check is on by default. Turn it off in
Settings > System > Overview > Check for updates automatically, or set
`enabled = false` under `[release_check]` in `~/.lucidos/updates.toml`. The
change takes effect immediately. The **Check for Updates** button still works
while it is off.

Dev builds launched from a source checkout skip the check.

### Release notes

Opening **Settings > System > What's New** downloads the project's published
changelog, so the panel can show releases newer than your copy. The request goes
to `raw.githubusercontent.com/lucidos-dev/lucidos/main/CHANGELOG.md`. GitHub
sees your IP address, under GitHub's privacy policy. It is a plain download of a
public file, with no workspace data, usage data, or version attached.

The download happens only when you open the panel, and Lucidos reuses the answer
for hours. If it fails, the panel silently shows the release notes bundled with
your copy. So the panel works offline.

### Plugins and plugin marketplaces

Plugins live in **git repositories**. Installing a plugin clones or fetches the
repository its `source` points to. Adding a plugin **marketplace** registers a
git repository that Lucidos can list plugins from. Once you register a
marketplace, Lucidos polls it in a periodic background check. It then
**auto-updates** installed plugins from their source when a newer version is
published.

The marketplace registry starts empty. Plugin network traffic starts when you
install a plugin or add a marketplace.

## Telemetry: there is none

Lucidos collects **no telemetry**: no analytics, no usage statistics, and no
crash or error reports, to us or anyone else. Its only network traffic is the
activity described above:

- LLM, tool, coding-agent and plugin calls. Each serves a task you started, or
  polls a source you configured.
- The gateway's hourly [update check](#update-checks) to `lucidos.dev`. It sends
  your platform, architecture and version, and you can turn it off.
- The *What's New* panel's changelog download, when you open it.
- The service worker's checks against **your own local engine** for a fresh
  frontend build.

The update check is the only one of these that reaches a server we operate.

## Questions and reports

For questions about this document, open a
[GitHub Discussion](https://github.com/lucidos-dev/lucidos/discussions). To
report a privacy or security **vulnerability**, such as a data leak this
document says should not happen, use the private disclosure process in
[SECURITY.md](SECURITY.md). Do not open a public issue.
