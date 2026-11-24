---
name: Lucidos Glossary, Core terms (continued)
description: Part of Lucidos Glossary. Its index lists the sections it covers.
---

One part of `system-knowhow/glossary`, which lists every part.

## Core terms (continued)

### Chat thread
A *thread* whose `source = 'chat'`: the user typed the opening message in the Lucidos chat UI. Answered by the *Lucidos Agent*. Contrast with *trigger thread* and *coding-agent thread*.

### Home thread
One *chat thread* per workspace, titled "Home", that never ends: **Home's chat**. It is a plain chat on every layout, and the *dashboard* is not part of it. **Every workspace has one**, and there is no switch for it. On a new install, Lucidos opens Home first, with the welcome in the chat. A device that has opened a thread before opens that thread instead. It can be neither archived, deleted nor pinned.

**Left alone, it starts nothing.** Home works only when you write in it, call it, or something it started for you comes back: a thread it started, or an event it waits for. It starts no work by itself, and asks for nothing.

**It starts a thread for work.** Home answers a question or a quick lookup itself, and gives anything more its own *sub-thread*. So the work runs in parallel, and Home stays free for your next message. A conversation that needs your answers, such as the *setup interview*, stays in Home. A sub-thread's findings reach you in its own thread, so a thread Home starts has the *wake mode* `no-wake` unless Home asks for more.

Its report is a *filed report*, which Home reads on its next turn, and *Review* lists the thread when it is worth reading. A failure is filed too: the sub-thread shows it in Blocked.

**You open it from Home, not from the thread list.** The Home icon leads the thread pane's header, with New thread beside it, and opens it. On a phone it sits beside the Lucidos mark, and the menu that the thread drawer's mark opens carries Home as its first row. The *thread drawer* draws no row for it, but its *sub-threads* show in the drawer's sections as threads of their own.

A widget you want in view goes on a *dashboard*. A tap on Home opens it. On a desktop it opens only the chat, even in Home, since the dashboard has its own rail and toggle. On a phone, a tap in Home slides the chat down to the dashboard.

**Only you name it.** You can rename it by hand, from its menu. Lucidos never names it: it gets no automatic title and offers no suggested name, because one topic in a thread that never ends is not its name.

**It reaches further than any other thread, with no more authority.** It reads any thread, and sends a *child follow-up* to any thread, *coding-agent threads* included. A thread it never spawned reports to its own parent, not to Home, so read its reply afterwards. It presses a button that belongs to you only while your words in that turn ask for it. That covers Apply, Archive and a permission card on another thread. The two widest "Always allow" grants are never an agent's to press.

*Voice sessions* live here and nowhere else.

**It also holds the cost of model calls no thread made**, such as a memory summary of a file written outside any thread. So the Token Cost app counts every call. Those calls run whether you use Home or not.
See also: *dashboard*, *voice session*, *child follow-up*, *standing instruction*, *setup interview*, ADR 0362, ADR 0381, ADR 0411, ADR 0432, ADR 0455, ADR 0496.

### Child follow-up
A message from a *parent thread* to one of its own *child threads*, sent with the `follow_up_child_thread` tool. The one privileged cross-thread write: redirect a child, hand it something a sibling learned, or tell a stalled child to continue. A thread can address its **direct** children and nothing else: not a sibling, not a grandchild (go through the child), and nothing cross-workspace. The engine looks the relationship up from the child's `parent_thread_id`. The one exception is the *home thread*, which may address any thread in its workspace.

A follow-up returns once the message is on the child's timeline and does **not** wait; the child reports back when its turn ends. It creates no thread, and the child cap never refuses it, so reviving a child is cheaper than spawning one. The revived child counts as a *live child* again while it runs. By default it **queues**: a mid-turn child reads it at its next natural break. To stop the child's current turn instead, see *urgent follow-up*.
See also: *child thread*, *parent thread*, *urgent follow-up*.

### Held message
An agent-sent message a *coding-agent thread* keeps back while it waits on a human: an open question, a pending permission card, or older held messages. Usually a parent's *child follow-up*, whose result reports `held`. The transcript shows it dimmed, saying it is not delivered yet, and the question stays answerable. On delivery, the delivered message replaces the held row, naming its sender and saying it was held.

An answer to the question or card releases held messages, oldest first, as does the user's next message or Continue. A Cancel keeps them held, since Cancel means stop. Events: `MessageHeld`, then `HeldMessageReleased` and the ordinary `MessageReceived`, which commit together. See ADR 0256.

A callback that lands under an open question dims too, on any thread: a returned child or an event-wait delivery. It says the agent has not read it yet. It is queued rather than held: the agent reads it right after the answer (ADR 0255). A queued message of yours dims too while a question blocks it.
See also: *child follow-up*, *read marker*.

### Queued card
A question card that waits because its thread already shows one. A thread shows one card at a time, in every client. A later card is recorded as `UserQuestionQueued`, so nothing draws it, pushes it or reads it aloud. When the open card is answered, canceled or superseded, Lucidos posts the oldest queued card with the same id.

You cannot answer a queued card, since you never saw it. A card from an agent turn that has moved on is dropped as superseded instead of posted. A card Lucidos raised itself waits until it is posted. Agent permission prompts never queue: the agent's call waits until the thread shows no card, then asks. See ADR 0755.

**When a card is open.** A card Lucidos raised stays open until you answer it, or until Lucidos retires it because what it asked about is gone. The end of a turn never closes it. An agent's own card stays open until you answer it or its turn ends. The engine and every client read this one rule (ADR 0803). A thread from before this rule can hold more than one open card: you answer them oldest first, and the later ones read "Waits for the earlier card".
See also: *home thread*, *held message*.

### Held idle proposal
A *coding-agent thread* that ended its turn while waiting on an event, with work on its branch that no *change* carries yet. The agent is still working, so the engine holds the proposal until the wait re-opens the thread and the agent finishes (ADR 0395). Until then the thread cannot be archived or deleted. **Stop waiting** proposes the work at once.

### Child thread
A direct descendant *thread* created by a `relation: "child"` spawn (`run_thread` / `run_coding_agent` / `lucidos spawn-thread --relation child`). When the child terminates, it reports to the *parent thread*, so the parent never waits. Its *wake mode*, set by the spawn, says whether the report resumes the parent. Under `no-wake` nothing does, a failure included: every report is a *filed report*. Under the other modes a failure always does, and a clean report follows the mode. A child spawned before wake modes existed has none, and its *wake decision* says it instead.

A crash that cuts the child's turn reports it as `interrupted`, since nothing resumes it. A child is a *sub-thread*; the reverse isn't true. A *child follow-up* from the parent earns another report when that turn ends.

Identifiers: the child row's `parent_thread_id` points up to the parent. `child_thread_id` names the child, on the `Callback` struct and the `ChildThreadCompleted` and `ChildReportFiled` events alike.

**Two ends of a turn send no callback.** A user Stop makes the child a *stopped child*, and its parent gets a note instead. A turn that ends holding an *event wait* sends nothing, because the turn the wait wakes reports.

A child stops being one when it is *moved to top level*.

### Command guard
A safety gate over the *Lucidos Agent*'s shell/Python tools, under **Settings → Permissions → Command safety**. It is on unless the workspace turned it off. A fast static check settles the obvious cases, and a cheap LLM **judge** decides the ambiguous middle, erring toward asking. Most commands run untouched, including reads anywhere and writes inside the workspace.

Three lanes. A clearly catastrophic command (`rm -rf /`, a fork bomb, formatting a disk) is refused. An irreversible real-world side-effect, or destruction outside the workspace, pauses on a *command permission card*. An in-workspace deletion or overwrite runs after a *command checkpoint*, leaving a one-click Undo.

Two sub-settings apply while the guard is on. Turning off the **LLM judge** uses the static dangerous-command classifier and destruction scan, and a command it cannot see through asks. If an enabled judge fails, the permission card asks the user and explains why. The **judge model** lives in Settings → Models and defaults to `gemini-3.1-flash-lite`, then `gpt-6-luna`, then `claude-haiku-5-5`, whichever a provider serves. It never runs on the chat model: with no judge model served, a turn under the guard and its judge refuses to start and says how to fix it.

A third sub-setting, **Answer coding-agent questions** (on by default), lets the guard also answer the permission cards of coding-agent commands (Codex, Claude Code `Bash`). It sees only what would otherwise card: the Claude Code permission mode and the Coding agent permissions list settle the rest first. On, a command the guard places as safe or reversible runs without a card, and a wrong reversible verdict has no undo. Off, the guard runs no Claude Code command unasked. Whatever the switches say, it refuses a catastrophic command it recognises. A Codex card means Codex's own sandbox blocked the command, so one the guard places as safe runs whatever the switches say.

"Always allow" commands live in an editable list under **Settings → Permissions → Lucidos Agent permissions**. The list is **per workspace**, so another workspace asks again. Lucidos imports the legacy `agent-allowed-commands` file at boot and deletes it after saving the grants.

A *trigger* fires unattended, so it cannot be asked. It runs irreversible commands only within its *program grants*, and an ungranted one is blocked and fails the run. See `system-knowhow/running-python/pick-the-right-tool.md` § The command guard.

### Command checkpoint
A pair of snapshots of the workspace's tracked content, taken by the *command guard* around an in-workspace delete or overwrite (the "reversible" lane). Instead of asking, the guard snapshots, runs the command, snapshots again, and shows a card with **Undo** and **Diff**. The two snapshots show exactly what the command did. Undo restores what it deleted or overwrote **and** removes files it created, leaving alone any you edited since. If the command changed nothing visible (a git-ignored target the snapshot never captured), no card appears. Snapshots are kept 30 days, then reclaimed.

Out-of-workspace destruction can't be checkpointed, so it takes the *command permission card* lane. Only taken while the *command guard* is on.

### Command permission card
The approval card the *command guard* shows when a shell/Python command needs the user's go-ahead. Same UI as the *coding-agent permission card*: Deny, Allow once, and a **Remember** menu. The menu holds Allow for this thread, Always allow (remembered for similar commands) and Always deny, which sets a *standing deny*. The thread waits until answered. Unlike the coding-agent card, "Allow for this thread" here is forgotten if Lucidos restarts.

### Compose destination
The compose view's single "who/where" picker for a new *thread*. It is the *Lucidos Agent* (default, a *chat thread*), or a coding target that spawns the matching *coding-agent thread*: the Lucidos source, an *app*, or a registered *repository*. A one-line caption states the pick's consequence. The Lucidos Agent can hand off to a *coding agent*. A coding target produces a reviewable *change*, except an *external-repo coding-agent thread*, which reviews the diff from the thread.

The Claude Code vs Codex pick is a separate chip, shown only for coding targets. It is remembered per workspace (`coding_agent_default` preference) and locked at the thread's first message.

The Lucidos source target appears only on a dev build with a source checkout. A packaged install hides it, gated on the `/health` `packaged` flag. An `.app` bundle remains packaged when built inside a checkout. The *Lucidos Agent* follows the same signal: its system prompt states whether platform source exists. `run_coding_agent` with `folder` omitted is refused without it, so picker and agent always agree.

### Config
Workspace configuration files under `data/config/`, principally `apis.json` (proxy entries, signer wiring, OAuth flows). Users edit these directly or via the engine's auth-handshake flow.
See also: `system-knowhow/building-an-auth-handshake.md`.

### Connected account
A service the user has signed in to, so Lucidos can act on their behalf: the
stored result of an OAuth authorization (access token, refresh token, granted
scopes, and the account's email where reported). Listed under
**Settings → Accounts → Connected accounts**, one row per provider. Created by
the *Lucidos Agent*'s `connect_oauth_account` tool or that page's Connect button.
Both open the provider's authorization page in the user's configured browser
(in-app panel, system browser, or new tab) and store the tokens on return.

A connected account is a **sign-in**. The OAuth Client *credential* beside it is
the **app registration** (`client_id`, optional `client_secret`, endpoint URLs)
that made it possible. So one provider shows one row in each list, which is not
a duplicate. The Connect flow creates the registration, prefilled from the
*OAuth provider registry*, and saving it continues straight into the browser. A
provider name may be a *derived provider*.

It records **two** scope sets: **granted** and **asked for**. They differ when a
provider refuses part of a request, which is a real state, not an error. An
example is a Dropbox app whose Permissions tab lacks a scope. *Reconnect*
re-requests the asked-for set, since the granted set holds only what the account
already has.

Backup uploads read the connected account for their `backup_provider`. The Backup
page links here, handing over the provider AND the scopes an upload needs, so one
authorization covers both.
See also: *credential*, *OAuth provider registry*, *derived provider*,
*OAuth client type*, *OAuth redirect URI*,
`system-knowhow/oauth-providers.md`.

### Connected-but-hidden
A device whose Lucidos page is alive (SSE EventSource streaming) but not *active*: another tab is selected, the window is behind another app, or the iOS PWA is in the app switcher. It receives the `NotificationCreated` SSE message and updates its bell badge silently, with no toast. Eligible for an *OS surface* push, subject to global suppression in §2 of `system-knowhow/notifications.md`. Distinct from *Offline*, where there's no SSE at all.

### Content pane
The pane where an opened thing lands and runs: an *app*'s *app UI*, a file or *artifact* preview, a *change*'s diff, settings, a URL. The single pane filling the *Canvas* side, third of the three panes beside the *thread drawer* and the *thread pane*. CSS container `.pane-content` (`FocusedPane = 'content'`; on mobile the rightmost swipe pane, `MobileView = 'content'`).
See also: *Canvas*, *thread drawer*, *thread pane*.

### Conversation
The left-hand side of the side-by-side desktop layout, where you talk with the *Lucidos Agent* to direct the work. The Conversation is where intent is expressed; the *Canvas* is where the result lives and runs. Like Canvas, it names a **side**, not a pane, but it covers **two** panes: the *thread drawer* (your threads) plus the *thread pane* (the open thread's transcript and prompt input). On mobile you swipe between Conversation and Canvas.
See also: *Canvas*, *thread drawer*, *thread pane*, *Live co-creation*.

### Current section
The *thread drawer* section holding the live working set: every *thread* not pinned, on hold or archived. That covers running threads (shown by the *Active* row indicator), threads awaiting the user, and recently idle ones. A thread never jumps sections as turns pass. Current is ordered by creation time, newest first, and never reshuffles as agents work.

Threads that need you are NOT bubbled up. The *Ongoing grouping* lists them instead: *Blocked* for an awaited answer or permission or a failure, *Review* for a ready *change* or a *read request*. The Blocked count rides the drawer header's grouping button and the thread-drawer toggle badge.

The drawer's other sections are Pinned, *On hold* and Archive. "Pinned" is the user-facing word; the section key `is_saved` and the `ThreadSaved`/`ThreadUnsaved` events still say "saved".

### Context window
How many tokens a *model* can hold in one request, prompt plus reply. The engine
reserves room for the reply, then trims the oldest *conversation* history and the
largest tool results until the rest fits. A window set too low throws away context
the model could have held.

Each *model registry* row may declare its window (Settings → Models, in the model’s Context window field). Left blank, the engine guesses from the model id. The guess knows only
Claude and GPT-5 ids and treats everything else as 200k. So an OpenRouter, xAI,
Gemini, or local model is under-budgeted until you set it.

Every guess errs low on
purpose: too low only trims early, while too high builds a prompt the provider
rejects. Builtins declare theirs where it could be verified.
See also: *Model registry*, *Provider*.

### Credential
A secret Lucidos stores on the user's behalf: an API key, bearer token, username
and password, mailbox password, a plain **secret**, or an OAuth client
registration. Listed under **Settings → Accounts → Credentials**, keyed by a
**service name**. A subprocess gets it as `CRED_<NAME>`, plus an optional
custom env var name as an extra alias. It does so only when its text names
one of them, a *secret mention*. The proxy auth pipeline
(`data/config/apis.json`) also resolves it to sign outbound requests. Distinct
from a non-secret *environment variable*.

Every secret you enter lives here and nowhere else, mailbox passwords and MCP
server env values included. A secret field may hold a *secret reference* to
1Password instead of the value.

A credential is identified by its service name **together with its auth type**.
That matters for one pair only: an OAuth Client registration may share a name
with an ordinary credential. So `google` can be both an API key and the Google
app registration, two rows told apart by their type badge. Every other name is
unique, because `CRED_<NAME>` and `apis.json` resolve it. The two engine-owned
types once used an `oauth:` / `email:` name prefix. The type carries that now,
so the name is just the provider or the mailbox account.

An OAuth Client is the one type NOT injected as `CRED_<NAME>`: only the OAuth
flow reads it, from the database. A secret is never a *preference*.

**The `secret` type names no transport.** Every other type says how the value is
sent (a key, a bearer token, basic auth, an app registration). A `secret` is a
shared secret something signs with, so it takes no base URL and no header. A
*webhook* signing secret is one; generating it in **Settings > Webhooks** saves
it here.
See also: *connected account*, *environment variable*, *config*, *webhook*,
*secret mention*, *secret reference*.

