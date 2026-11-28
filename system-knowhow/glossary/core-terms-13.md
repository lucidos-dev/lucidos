---
name: Lucidos Glossary, Core terms (continued)
description: Part of Lucidos Glossary. Its index lists the sections it covers.
---

One part of `system-knowhow/glossary`, which lists every part.

## Core terms (continued)

### Update address
Where this machine's update check looks for a new Lucidos, as a `latest.json` URL. Empty means the published release, which is the default. A developer sets another one to test a release before it is published. It must use `https`, or `http` to `localhost`, `127.0.0.1` or `[::1]`.

One setting covers the whole machine. The gateway's hourly check and the macOS
app's **Check for Updates** both read it. Save and select a custom slot with
`lucidos update-channel add Preview <url> --use`, or edit it in Settings.
Selecting Stable keeps every slot. A refused address is an error, with no
fallback. Every installable update still needs the Lucidos signature.

### Urgent follow-up
A *child follow-up* marked `urgent: true`, which stops the child's current turn so it reads the message now, not at its next natural break. An ordinary follow-up queues, because a steer should never throw away an in-flight build. Urgency loses whatever the interrupted turn was mid-way through. So use it for messages that cannot wait (a cancellation, "stop, you are working from a wrong assumption"), not for hurry.

Without it the wait is unbounded: a child in a long tool call reads a queued message only when the call returns. A *coding-agent thread* in a ten-minute blocking wait really sits on a STOP for ten minutes. The interrupted turn ends as "Superseded", not "Canceled", since the work is steered, not abandoned. So the parent gets no false completion card for a child that carries on.

Two caveats. A child parked on a question waits on a *human*, so urgency cannot unblock it. A **Codex** child on an app-server with steer support takes ordinary follow-ups during its turn. Older app-servers and the exec protocol still interrupt for every follow-up. Ask for what you mean, whatever the child's backend.
See also: *child follow-up*, *coding agent*.

### Call toggle
The handset button in the prompt input's row that starts and ends a *voice session*: press to call, press again to ring off. It is green while a call is up and grey while hanging up. Point at it and it turns red, which is what the next press does. A slow connect says it is waiting for microphone access, which the browser asks for once per launch.

It is a toggle, not a hold-to-talk microphone: the microphone stays open for the whole call. It sits in the *home thread*'s prompt input, the one place a call runs.

**Holding it picks which microphone a call opens**, from those this browser can see. Otherwise a call takes the system default, which on a Mac can be a nearby iPhone. The choice is per device and applies from your next call.

A live call keeps the control visible so you can always hang up. Start a call
from the home thread with the Lucidos Agent. Voice ships off. Turn it on in
Settings → Models → Voice.
See also: *voice session*, *home thread*.

### Voice session
Talking to Lucidos out loud in your *home thread*. Voice is a **mode of a thread**, not a kind: the same thread, history and agent. The composer stays live, so you can speak a sentence and type the next, both in the same conversation.

It starts and ends at the *call toggle*. It also ends when you open another thread, when the connection drops, or when the engine goes away.

**A call runs on the home thread, and nowhere else.** Every other thread offers no call: the handset is absent, and the engine refuses one placed any other way. From Home, Lucidos can still reach the rest of your work: it reads any thread and follows one up when you ask.

During a call the handset stays green; there is no separate panel. What each of you says lands in the thread as it is said. Your bubble appears as you start speaking and pulses until your words arrive. Speaking over the assistant stops it mid-word, as on the phone.

**Not in a workspace opened through another gateway.** That gateway relays pages and requests, but no call. Settings → Models → Voice says so there, with no switch.

**Experimental, and off until you turn it on** at **Settings → Models → Voice**. The voice page also holds the call's two models, its voice, and what it loads before starting. One model talks; the other turns your speech into text. A call with no talker says so, with a button straight there.

**Speaking wakes the thread**, unless Lucidos answers you itself. A sentence it hands on lands as a message and starts an ordinary turn, as typing would. One it answers from what it knows starts nothing, but is still written down.

**You can answer out loud what Lucidos is waiting on.** A pending question or permission is read to you, and saying your choice settles it, writing what the on-screen buttons write. A permission card reads every button, each "Always allow" naming the line it stores. An "Always allow" is settled only by your own words, and only when they clearly ask for that reach, such as "always allow git". The talker never picks one for you. While something waits, no new request starts: Lucidos is stuck inside the question, and says so.

**How much of that works depends on the talker.** A Realtime one settles everything out loud. GPT Live holds no tools, so it settles a question by sending your words verbatim, as typing a reply would. There a permission is a tap, and so is ringing off. The talker row in Settings says which you are on.

**Saying you are done ends the call.** "That's all, thanks" rings off after the goodbye, and running work keeps running.

**The audio is thrown away, the words are not.** Nothing is recorded. The thread keeps what you said, what the assistant answered, and what was said out loud. It also keeps when the session opened and closed, and what the call spent.

**The transcript says which of it was spoken.** A spoken message carries a small handset above it, so a half-spoken, half-typed conversation reads back correctly. The bubble is the one a typed message gets, and the mark appears either way, whether the assistant answered at once or went to look. A spoken answer to a question it asked is written on the card as the answer itself.

**What the assistant said out loud is a bubble inside the turn**, under the same handset, marked as cut off if you talked over it. It sits beside the written answer, not instead of it.

**What you hear is not what you read.** The written answer is for reading, and can carry tables, code and links. The spoken reply is what it *means*, in a sentence or two. A long answer is spoken as the short version plus an offer of the detail. The full text waits in the thread.

**One entity, honest about what it knows.** You hear one assistant, in the first person. What it answers instantly is bounded by what loaded when the session opened. For anything else it says it is checking, and comes back. It never claims to have done something it did not do.
See also: *chat thread*, *thread*.

### Wake question
An `ask_user_question` called with exactly one option. The *Lucidos Agent* uses it to offer one tap-to-send follow-up, like "Show results" or "Stop sweep", instead of a typed wake message. The agent's context lives in the `question` text; the option's label is the user-voice prompt the tap sends.

The thread shows the "?" attention status (`WaitingForUserAnswer`) until the user taps. A free-form message also auto-resolves the pending question.
See also: `system-knowhow/running-python/pick-the-right-tool.md` § The drain pattern.

### Quiet option
A question-card option that needs nothing more from the agent, such as "Leave it as posted" or "Not now". Picking it records the answer and ends the turn: the agent does not wake, so it writes no reply and does no more work. An option that promises work is therefore never quiet, and a card whose every option is quiet is refused. Typed text beside a pick always wakes it, and only a one-question ask can carry one: a later card still needs the agent. The Lucidos Agent and Codex mark one with `quiet: true`, and Claude Code with a `[quiet]` label prefix that the card never shows.

A quiet option may also carry a *quiet reply*.
See also: *quiet pick*, *quiet reply*, *wake question*.

### Quiet pick
An answer that picks only *quiet options* and types no text beside them. It ends the turn rather than waking the agent.

### Quiet reply
A fixed acknowledgement a *quiet option* carries, written by the agent when it asked, such as "Then everything is set up correctly." A *quiet pick* shows it as the agent's final reply, and no model call writes it. The Lucidos Agent and Codex write it as `quiet_reply`, and Claude Code as `[quiet: <reply>]`, which ends at the first `]`. A pick of several quiet options shows their replies in option order, one per paragraph. Only a quiet option may carry one. A Claude Code or Codex agent that is no longer running shows no reply, and neither does a turn the user stopped.

### Release notice
What a release asks you to know or do after upgrading. A release has one notice, with a summary for each item that applies to the workspace. Most releases carry none. The release notes in *What's New* describe what changed.

On open, the modal shows owed releases oldest first. **Read more** expands an item's details. **Got it** answers the items shown, and **Check my workspace** starts one Lucidos Agent thread with the checks and requests from those items. A successful start also answers them. **Later**, Escape, the X and following a link leave the items owed.

**Settings > System > Release Notices** keeps visible items available. Owed items show at the top, the same ones the modal shows. Answered items fold under **Already answered**, so a release you partly answered appears in both places. A fully answered release keeps its check button there.

The workspace remembers answers across devices. A brand new workspace starts level and hears from its next upgrade.

### Notice item
One change a *release notice* asks you to read or act on. Each item has a summary and can carry details, a workspace check or an action request. An item can apply only to workspaces that meet a condition. For example, the Tree memory item appears while a workspace uses Classic.

A newer item can replace an older item when the newer one applies. The old item then disappears from the modal and Settings, even if you answered it. Work you already started stays in its thread.

### What's New
The release notes panel, at **Settings > System > What's New**. It lists every published Lucidos release newest first, with the running one open and marked **Running**, and the rest as expandable rows. The notes are the project's changelog prose.

Opening the panel reads the published changelog, so releases newer than your copy are listed too. When it cannot be reached, the panel falls back to the copy **inside the engine binary**. That keeps it working offline and on an install with no source checkout. The fetch happens on panel open, never on a schedule.

A release the updater is **offering** sits above that list, with the control to get it. With no offer, the control sits on the newest release ahead of you, since the changelog arrives before the update check. On the desktop app the control is **Update & Restart**, and the row wears no chip. A browser or phone has no updater, so the row keeps its **Available** or **Newer** chip and offers **How to Update**. That opens the page for your kind of install.

One exception applies when the desktop app is open on the Mac that runs the workspace. Then a browser or phone gets **Update Desktop App** instead, even for a release published a minute ago. Lucidos checks for the release when you tap. The desktop app installs the release and restarts Lucidos, while your phone shows progress. It works only where nobody needs to be at the Mac. So it is not offered from the disk image, or from a folder that needs an administrator password.

An offered version is newer than the running binary, so its notes are not in that binary's history. They arrive with the update check. So the offer's notes appear only where a real update is pending, and never fall back to the installed list. That would show your CURRENT version's contents under the heading of the one you were about to install.

Three ways in. The Lucidos menu's version row opens it (tap the Lucidos mark in the header). That row carries a dot until this device reads the notes for the release it runs; the dot is per device. The update notice on Settings > Updates & Installs links here too, as does the update toast, next to Update & restart.

The two update links open the release they announced, since they answer "what is in it". They open the panel at the top, so an offer never lands you mid-list. Every other way in opens the release you are running.

### System attention badge
A small blue dot on the way into **Settings**, saying something there awaits you. It appears at every step in: the hamburger that opens the menu drawer, that drawer's **Settings** row, and the row of the page that holds the work.

Two things raise it, on two pages. A Lucidos update you can take dots **Settings > Updates & Installs**. An unanswered *release notice* dots **Settings > System** and its **Release Notices** row. Both are work, which separates the dot from news you might simply read. Hovering the hamburger says which.

Upstream of those pages one dot stands for both. On a Settings row it means work on that row's page, so an update never dots System.

**It clears by being acted on, never by being seen.** Install the update or answer the notice and it goes. Opening and closing the panel changes nothing, nor does closing the release notice card.

Distinct from the dot on the Lucidos menu's version row, which means you have not read the notes for the release you run. That one is per device and clears when you open the panel. This one is about the workspace, and about work.
See also: *What's New*, *release notice*.

<!--gloss-webhook-start-->
### Webhook
An endpoint you point a third party at, so their service can tell Lucidos something happened. Each webhook emits exactly one *domain event*, which a *trigger* can then react to. Manage them at **Settings > Webhooks**, or with `lucidos webhooks`.

**The event is pinned when you create the webhook**, and no caller can change it. So an endpoint you gave GitHub only ever fires the event you chose, whatever GitHub posts. For several kinds of message from one sender, fire one event and discriminate in the trigger's `condition` on the sender's own fields.

**Only you create one.** No agent tool or app can, for the same reason an agent cannot grant itself a *program grant*: a webhook opens a door, and an agent must not widen its own authority.

A delivery has to prove itself. An unsigned webhook carries a **token**, shown once at creation, that the sender returns as `Authorization: Bearer <token>`. Or it carries a **signature** configuration, as GitHub, Stripe and Slack sign the request body with a shared secret. Those senders attach no token, so a signed webhook gets none. The secret is a *credential* you save once and name here, never a copy beside the webhook.

**The sender decides which side invents that secret**, and the form follows. GitHub takes whatever secret you put in its webhook form, so Lucidos offers to generate one and shows it once for pasting. Slack and Stripe issue their own in their console, so you paste those in. Either way you may name an already saved credential instead.

**A signature can be changed or removed without changing the URL**, so a rotated secret or wrong signature header is fixed in place. A webhook carries exactly one of the two verifiers. Adding a signature drops the token; removing one mints a fresh token, shown once.

A delivery becomes an event shaped `{summary, headers, payload}`. The sender's body is under `payload`, so a trigger condition reads `payload.action`. Allow-listed request headers are under `headers`, read as `headers.X-GitHub-Event`.

**A sender will resend the same delivery**, and by default that emits the event again. GitHub retries a slow response and offers a Redeliver button; Stripe retries for days. Every arrival stays on the event log, so you can see how often a sender repeats. Switch on *delivery deduping* to count each delivery once.

Deliveries arrive on their own port, the *hook socket*, which answers webhook URLs and nothing else. That makes it the one surface safe to expose to the public internet with `tailscale funnel`.
See also: *trigger*, *domain event*, *credential*, *delivery deduping*, `system-knowhow/lucidos-cli/subcommands-15.md` § `lucidos webhooks`.
<!--gloss-webhook-end-->

### Delivery deduping
A per-*webhook* setting that makes a resent delivery emit nothing instead of firing the event a second time. **Off by default**, so every arrival is an event unless you ask otherwise. Set it with `lucidos webhooks`, which is also where a signature is configured.

You name the header carrying the sender's delivery id (`X-GitHub-Delivery` for GitHub) and a window. A repeat of that id inside the window emits nothing. It gets a success and the first delivery's event id, so the sender stops retrying. A repeat arriving while the first is still being handled waits for it, and is delivered instead if the first fails. Name no header and the body itself is the key, which also collapses two different but identical-looking deliveries.

The window defaults to an hour and can go up to seven days. Setting it to `0` turns deduping off again.

Only *this* endpoint's deliveries are compared, so two webhooks fed by the same sender both fire, as two subscriptions should.
See also: *webhook*, *domain event*.

