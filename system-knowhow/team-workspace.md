---
name: Team workspace setup
description: A team's shared factory workspace. "team workspace", "set up Lucidos for my team", "offboard someone". Hosting, people, bots, team keys, gate policy, Factory app, ticket and chat intake, @mentions, agent pause.
---

# Team workspace setup

A *team workspace* is one Lucidos workspace that a whole team pairs devices to. It holds the credentials and does the work. Tickets and chat messages start *root threads* on their own, some finish as a pull request, and some stop at a *gate* for a person. The design is ADR 0623. The terms are in the glossary: *operator*, *root thread*, *driver*, *continue key*, *actor*.

Walk the user through the steps below in order. Ask before each step that creates an account, a credential or a public address.

## Who this suits, and who it does not

It suits a small team that trusts each other with the same powers. Every paired device can do everything: a person is identity, never permission.

It does not suit:

- **A team that needs person-level permissions**, or commits signed as the person. The factory commits as its bot.
- **Intake from people outside the team.** Each fire runs with the factory's credentials on one host. A sandbox per thread is deferred until intake opens to outsiders, so only teammates may start work.
- **People whose source email is not on their person.** They read as strangers until someone adds the email.

## 1. Host it

Run one workspace on a machine that stays on: a Linux server with the headless install, or a Mac that does not sleep. Reach it over a tailnet or a VPN the team already has, as `remote-access.md` describes. Behind a company sign-in proxy, set the *external address* (`lucidos-cli.md` § `lucidos external-address`). The proxy decides who reaches the page. Pairing still decides who gets in.

Never expose the gateway's main port to the internet. Only the hook socket may be public, and only when a webhook template needs it.

## 2. People and their devices

In **Settings → Devices → People** (`remote-access.md` § "People: whose device is which"):

1. Add a person per teammate, with every email they use in the issue tracker and the chat tool.
2. Pair each device from **Settings → Access → Pair a device**, choosing whose it is.
3. Check each device row names the right person. One device belongs to one person; shared devices are not supported.

Emails are how a comment or a message is credited to a person. Matching is exact after trimming and lowercasing.

## 3. Credentials belong to the factory

Every credential the workspace holds is the team's, not a teammate's:

- **Bot accounts** for each source, each limited to what the factory touches. The source templates below set them up.
- **Model keys**: a team account for each model provider, in **Settings → Providers**. Never a teammate's personal subscription: it leaves with them.
- **Coding agents**: log Claude Code or Codex in on the host with the team account, and `gh` with the bot.

Each credential records who saved it. Nothing enforces this rule, so state it to the team.

## 4. The gate policy

A gate is a question card in a root thread or one of its sub-threads. Which steps stop for a person is the team's policy, and it lives in knowhow, never in the engine. Write it with the team to `data/knowhow/team/gate-policy.md`. A starting point:

```markdown
---
name: Team gate policy
description: When the factory must stop and ask a person before going on. Use for every root thread from the issue tracker or the team chat.
---

Ask with a question card, and wait, before you:
- merge anything, or push to the default branch;
- change a database schema, a public API, or the build and release setup;
- spend money, or contact anyone outside the team.

Open pull requests freely. Ask once, with options, when the request is
ambiguous. Never ask to confirm work that is done.
```

Then set the timeout after which a gate its driver left reaches the whole team:

```bash
lucidos preferences set --key driver_timeout_hours --value 24
```

## 5. The Factory app

The Factory app lists the open root threads with their driver, their watchers and their gates. **Mine** is what you drive or watch, **Team** is gates nobody owns, and **All** is everything open. **Take**, **Take over**, **Release**, **Watch** and **Unwatch** act as the person holding the device. It reads `lucidos.threads.people` (`js-sdk.md` § "lucidos.threads.people").

Copy these three files into `data/apps/factory/`, keeping their paths, with `lucidos data write`:

- `system-knowhow/team-workspace/factory-app/manifest.json`
- `system-knowhow/team-workspace/factory-app/index.html`
- `system-knowhow/team-workspace/factory-app/assets/icon.svg`

Then run `lucidos apps list` and check that **Factory** is there.

A Factory app copied before the rename calls the retired `lucidos.workItems` and shows an error. Copy the three files again after you upgrade.

## 6. Sources of work

Each source is a template of four triggers:

- a translation script that emits a domain event carrying the author's email;
- an intake trigger with a continue key and an actor;
- an outbound script that mirrors gate questions;
- an outbound script that posts outcomes.

The templates:

- `system-knowhow/team-workspace/issue-tracker`: issues and comments, through a signed webhook. GitHub is the worked example.
- `system-knowhow/team-workspace/chat-tool`: messages that mention the bot, polled. Slack is the worked example.

Both share `system-knowhow/team-workspace/source_thread.py`, which finds the source address of a root thread from its continue key.

The engine knows nothing about either source. To add another, copy a template and change its marked functions.

## How a root thread runs

- **Started by a person** in Lucidos: they are its first driver.
- **Started by a ticket or a message**: no driver yet. Its gates reach the Team view and push the whole team.
- **Answering a gate**, in Lucidos or from the source, makes the answerer the driver. An answer from a source counts only when its email is on a person.
- **A gate its driver leaves** past `driver_timeout_hours` escalates once and pushes everyone. The driver keeps the root thread.
- **Archive the root thread** to finish its work. Only Archive releases its continue key, so the next comment on that ticket starts a fresh one.

Anyone can still answer any gate, whoever drives.

## How people talk in a thread

Teammates can talk inside a thread without its agent answering, so the talk about the work stays beside the work (ADR 0649). The terms are in the glossary: *person mention*, *agent mention*, *agent pause*, *people-only message*.

- **`@Name` writes to a person.** The composer suggests teammates as you type `@`. **Tab** takes the first suggestion, and Enter still sends.
- **Mention only people, and the agent pauses.** The message reaches them as a push and stays out of the agent's way. Untagged messages stay between people until someone resumes it. A reply typed to an open question card still answers it.
- **`@agent` wakes the agent**, and ends the pause. So does **Resume** on the hint line above the composer.
- **The agent still reads everything.** Its next woken turn sees the people-only messages, each under its writer's name.
- **The People row** under the thread's title shows the driver and the watchers as initials. **Watch**, **Unwatch**, **Take** and **Release** sit there too. A mention makes someone a watcher, so they leave with one tap.
- **To start a thread with a teammate**, pick them under "A teammate…" in the compose destination. The first message then mentions them.

A trigger fire, a sub-thread's report and a question answer always wake the agent, paused or not. A gateway with one person shows none of this.

## Config changes

A change to a trigger, a repository, an MCP server or a credential tells the rest of the team. The notice offers **Undo** where Lucidos recorded enough to reverse it (`notifications.md`, "Config change notices"). A secret's value is never restored.

## Offboarding someone

1. In **Settings → Devices**, under People, use **Remove** on their person. That revokes every device they hold.
2. Rotate each credential they saved or saw, from **Settings → Accounts** or with the provider.
3. Remove them from the bot accounts' organisation and from the chat tool's app collaborators.
4. Their root threads lose their driver, so their open gates show in the Factory app's **Team** view. Take each one there.
5. Remove them from each translation script's `people.json`, the list of who may start work.
