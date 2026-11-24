---
name: Lucidos CLI (lucidos), Subcommands (continued)
description: Part of Lucidos CLI (lucidos). Its index lists the sections it covers.
---

One part of `system-knowhow/lucidos-cli`, which lists every part.

## Subcommands (continued)

### `lucidos threads follow-up --thread <child-uuid> --message <M> [--event-id <E>] [--urgent]`

Send a message to one of **this thread's own child threads**, or from the *home thread* to any thread: redirect one going the wrong way, hand it something a sibling learned, or tell a stalled one to continue. This is the *child follow-up* edge, the one privileged cross-thread write. Wraps `POST /api/v1/threads/<child>/follow-up`.

```bash
# Redirect the child that is taking the wrong approach.
$ lucidos threads follow-up \
    --thread 9c1f2b40-... \
    --message "Skip the CSV path entirely, the source is a live API."
{"child_thread_id":"9c1f2b40-...","reach":"own-child",
 "child_title":"Import the sales figures","delivered_to":"running",
 "detail":"The child was mid-turn, so this queues behind its current work or steers it."}
```

The ack prints as raw JSON on stdout, like every other `lucidos threads` subcommand. The chat agent's equivalent is the `follow_up_child_thread` tool, with the same reach.

- **You can only address your own DIRECT children.** No siblings, no grandchildren, no arbitrary thread. No flag says who you are: the engine reads the calling thread off the *thread-bound origin token* this subprocess was spawned with, then checks the child's own row. A thread that is not yours is a 403 whatever you claim.
- **The *home thread* is the one exception.** It may address any thread in its workspace, coding-agent threads included, and the ack's `reach` reads `home`. A thread it did not spawn reports to its own parent, not to Home, so read its reply with `lucidos events` afterwards. Every other caller's ack reads `reach: own-child`.
- **This is a boundary, not an obstacle.** Outside the home thread, if asked to message a thread you did not spawn, there is no other route. Say so, name the threads, and let the user send it (see the top of this file).
- **It returns as soon as the message lands, and does not wait for the child.** The child reports back the usual way, as a completion card on its parent. The ack's `delivered_to` says which of five things happened:
  - `running`: the child was mid-turn, so the message queues behind its work or steers it.
  - `interrupted`: you passed `--urgent`, and the child's turn is stopping so it reads you next.
  - `waiting-for-user-answer`: parked on a question or permission card, so **a human must answer before it reads this**.
  - `held`: the child awaits a human reply; delivery resumes after that reply.
  - `revived`: it was not working, so a fresh turn starts now.

  `detail` says the same in a sentence.
- **`--urgent` is for cancellations, not for hurry.** By default a mid-turn child reads you at its next natural break. Inside a long tool call that can take many minutes: a child in a ten-minute blocking wait reads you when the wait returns. `--urgent` stops the child's current turn so it reads you at once, and that turn's unfinished work is lost. Use it when the child must act on your message *instead* of what it is doing.
- **Address the child by uuid, never by title.** Titles are not unique, and a fuzzy match would deliver to the wrong child. Find the id with `lucidos threads list --my-children`. Afterwards, refer to the child by the ack's `child_title`: a uuid names nothing the user can see.
- **A follow-up is never refused at the child cap.** The cap limits spawns, not messages, so reviving a child is cheaper than spawning another. The revived child counts as live again while it runs.

`--event-id` defaults from `$LUCIDOS_EVENT_ID` and stamps the child's message-route panel, so the follow-up links back to the originating event.

**A cancellation is not done when the ack returns.** The ack says only that the message is on the child's timeline. Even with `--urgent`, the child still has to read it and do the work of stopping. If you told a child to kill a job, verify the job is gone (no processes, no lock file) before reporting the cancellation complete.

### `lucidos threads detach --thread <child-uuid>`

Move a child thread to **top level**, so its parent stops waiting for it. Wraps `POST /api/v1/threads/<child>/detach`, the same move the thread menu's **Move to top level** makes.

```bash
$ lucidos threads detach --thread 9c1f2b40-...
{"child_thread_id":"9c1f2b40-...","child_title":"Import the sales figures",
 "former_parent_thread_id":"4b7e0c11-..."}
```

- **From inside a thread you can move only your own DIRECT children.** The engine reads the calling thread off the origin token, exactly as for `follow-up`. Anything else is a 403. Outside a thread, with no origin token, it moves any thread that has a parent.
- **It needs nothing else.** Moving your own child out creates a top-thread, and any thread may create one (ADR 0475).
- **Nothing is stopped.** The child keeps running, finishes on its own and proposes any change. Its result goes to its own timeline only.
- **The former parent gets nothing more.** No completion card, no follow-up, and it leaves `--my-children`. A card the child earned before the move still arrives.
- **It frees no child slot while the child runs.** The moved child counts against the former parent's cap until it finishes.
- **It cannot be undone.**
- A device caller gets 409 for a top-level thread. An agent gets 403 when the
  target is outside its direct children. A discarded child gets 409; an unknown
  id gets 404.

### `lucidos threads child-reports`

Read the reports your child threads filed that you have not read yet. Wraps
`POST /api/v1/threads/<id>/child-reports/take`, for the thread in
`$LUCIDOS_THREAD_ID`.

A child that finishes while your turn runs sends its report as your next
input, and your agent reads input only between turns. So the report waits
until the turn ends, even when you need it now. Your own `lucidos` calls carry
it to you instead:

- **Most `lucidos` calls note it on stderr**, in one line naming this
  command. The call's own output is unchanged.
- **`lucidos background-task wait` returns early** with the report under
  `child_reports`.
- **This command reads every waiting report now**, oldest first.

```bash
$ lucidos threads child-reports
{"child_reports":[{"card_id":"7d2e…","report":"[CHILD THREAD COMPLETED] …"}],
 "message":"Your children's reports, oldest first. They will not be delivered again."}
```

- **Each report reaches you once.** Taking it withdraws the queued copy, so it
  is not delivered again when your turn ends. A report your agent already read
  is not handed back, and an empty list means none waits.
- **Nothing interrupts your turn.** The report waits for one of your calls.
- Only the thread's own agent can take its reports.

### `lucidos threads archive --thread <current|child-uuid>`

Archive this thread, or one of its own direct children (ADR 0310). Wraps `POST /api/v1/threads/<id>/archive`, which runs the same cascade as the Archive button.

```bash
$ lucidos threads archive --thread 9c1f2b40-...
{"archived":["9c1f2b40-..."],"skipped":[],"summary_versions":{"9c1f2b40-...":12}}

$ lucidos threads archive --thread current
{"requested":"4b7e0c11-...","detail":"This thread is archived once its turn ends and it has settled."}
```

`summary_versions` gives each archived thread's row version once its archive landed. A client that moved the row early holds it until the row reaches that version.

- **From inside a thread you can archive only yourself and your own DIRECT children.** The engine reads the calling thread off the origin token, and resolves `current` to it. Anything else is a 403 with `not_your_thread`, an unknown id a 404, and a discarded thread a 409. Outside a thread, with no origin token, it archives any thread, as the Archive button does.
- **A child is archived now**, with its own sub-threads. **`current` is archived once this turn ends** and the thread has settled. A new message into it before then keeps it open.
- **The home thread is never archived**, by anyone: a 409 with `home_thread`.
- **The Archive button's refusals apply.** A running target or one waiting on the user is a 409 with `parent_not_archivable`. A pending change is `parent_has_pending_changes`, and a blocking sub-thread is `descendants_blocking`. A target parked on an event wait with unproposed work is `parent_holds_held_proposal`: stop waiting first. Each body carries a `message` saying what to do.
- **A pinned target is a 409 with `thread_pinned`**: only the user archives a pinned thread. A pinned sub-thread of the target stays open and is listed under `skipped`.
- **Archiving removes the worktree about an hour later.** Lucidos commits uncommitted work on the thread's branch first and keeps the branch. A later follow-up recreates the worktree on that branch.

### `lucidos threads context-ceiling <ceiling> [--thread <uuid>]`

Set a thread's *context ceiling* (ADR 0629): the context size at which its coding-agent session hands off to a *successor session*. Wraps `PUT /api/v1/threads/<id>/context-ceiling`. `--thread` defaults to this thread.

```bash
$ lucidos threads context-ceiling 200k
{"ceiling":"200k"}
```

- **The value** is `off`, a size such as `200k`, or a share of the window such as `30%`. No ceiling goes above 75% of the window or below 100k.
- **Only a person raises a ceiling or turns it off.** From inside a thread you may lower your own, and set a direct child's at or below your own. A raise is a 403 with `ceiling-raise-needs-a-person`. Any other thread is a 403 with `ceiling-thread-out-of-reach`.
- **A child takes its parent's ceiling at spawn.** A later change on the parent does not reach it.

### `lucidos threads hold --reason <why> [--thread <uuid>]`

Put a thread and its sub-threads on *hold* (ADR 0805): Lucidos starts no turn there by itself until the owner resumes it. Run it when told to checkpoint and pause, after committing. Wraps `POST /api/v1/threads/<id>/hold`. `--thread` defaults to this thread. `pause-automation` is a one-release alias.

```bash
$ lucidos threads hold --reason "Checkpoint before the migration"
{"held":true,"changed":true}
```

- **What it holds:** sub-thread reports, event-wait deliveries, the harden and plan nudges, the context-ceiling handoff and every other automatic re-entry. A held thread raises no card for any of them.
- **What it stops:** every running turn in the tree, except your own. Your own turn ends by itself, so your reply can say what you checkpointed.
- **What it sets aside:** every pending change in the tree, until Resume brings them back.
- **What still happens:** messages and answers arrive, and the owner's message runs a turn. The thread stays on hold.
- **Who may hold:** the owner, the thread's own agent, an agent above it, or Home. Home may hold any thread in its workspace. Any other thread is a 403, and an unknown thread a 404. Chat and coding-agent threads both hold. A repeat keeps the first reason and answers `"changed":false`.

### `lucidos threads resume [--thread <uuid>]`

Resume a thread on hold, and exactly the sub-threads its hold reached. Wraps `DELETE /api/v1/threads/<id>/hold`. `--thread` defaults to this thread.

```bash
$ lucidos threads resume
{"held":false,"changed":true}
```

- **Only the owner resumes**, with **Resume** on the hold strip. The thread's own agent may run this only inside a turn the owner's own message started there, on the owner's words. A parent, Home and a trigger get a 403.
- **Resume runs on the root**, the thread the hold was placed on. A sub-thread the hold reached answers 400 and says to resume the thread above it.
- **What it brings back:** the changes the hold set aside. Steps that came due are worked out again, never replayed. A sub-thread stopped or held before the hold stays so.
- **An unknown thread is a 404.** A thread with no hold answers `"changed":false`. Archiving the thread also lifts its hold.

### `lucidos threads create-draft --text <T> [--image <path> ...] [--mode lucidos|claude-code] [--repo <id> | --app <id>] [--coding-agent <backend>] [--model <M>] [--provider <P>] [--effort <E>] [--retry-key <K>]`

Hand the owner a ready message as a *draft* in a new thread, without opening it (ADR 0665). Wraps `POST /api/v1/threads/drafts`. Nothing is sent, no session starts, and the owner's screen stays where it is. The draft appears in their drafts list, and they send it, edit it or discard it.

```bash
$ lucidos threads create-draft --text "Here is the weekly summary." --image artifacts/reports/weekly.png
{"created":true,"retry_key":"4f0c...","thread_id":"9c1f2b40-...","title":"Untitled","link":"thread:myws/9c1f2b40-...","preview":"Here is the weekly summary.", ...}
```

- **The answer is the draft** as `threads drafts --thread` reads it, plus `created` and `retry_key`. Its `snapshot` carries the revision and epoch. Link the owner to it with `[title](link)`.
- **A retry never clobbers an edit.** The same `--retry-key` from the same thread names the same draft, and returns it as it now stands with `"created":false`. Omit the key and each call makes a new draft. A key whose draft was sent or discarded is refused.
- **Images** are paths under `data/` (`artifacts/...`), in order, up to 32. Each must be an image the composer takes.
- **`--mode claude-code`** makes a coding-agent draft. `--repo` takes a registered external repository's id and `--app` an app id; omit both for the Lucidos source. `--coding-agent` defaults to the owner's default backend.
- **`--model`, `--effort` and `--provider`** must be picks the send accepts. A chat draft takes `--provider`; a coding-agent draft does not.
- **Refusals write nothing**: a bad path, a non-image, an unknown model, or an empty draft is a 400 naming the reason. A caller outside a Lucidos thread is a 403.

