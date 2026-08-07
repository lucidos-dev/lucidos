# Philosophy

Vision, mission, and the lens that surface and integration proposals are held
against. They build in that order: the vision is the world we think is coming,
the mission is what we do about it, and the lens is how we choose between the
many ways of doing it. Each one should be derivable from the one above it.

<!--philosophy-start-->
## Why Lucidos exists

Two things are true at once about AI tooling today. The models are
extraordinary, and nearly every product built on them is a thin client in front
of somebody else's service. Your work still lives in fifteen places and the
assistant visits it, briefly, through a straw: a chat window with no memory of
your files, an editor plugin that sees one repository, a bot in a chat app that
owns the conversation and forgets it tomorrow.

## Vision

> **A person owns their data, including software, on their own devices.**

**Including software** is the part that is not yet obvious. Source is text, a
described app is a description, and the only reason software ever felt like a
different category from your documents is that you could not write it yourself.
That reason is going away.

**Owns**, not licenses. Not hosted on their behalf, not held at somebody's
discretion, not available until the terms change. And **on their own devices**,
because ownership you cannot exercise on hardware you do not control is a
courtesy, and courtesies get withdrawn.

That is not how personal computing turned out, and the reason was cost.
**Software wants to be free**, in Stewart Brand's sense rather than the price
one: free of an owner. He said it about information, and if software is data
then this is not an analogy, it is the same claim reaching further. What made
software somebody's product was that building it was expensive, so it had to be
built once and sold many times, which requires a seller. When describing a thing
is enough to make it exist, that reason is gone, and what remains is the part
nobody actually chose: a roadmap, a subscription, a sunset date, and someone
else's idea of what you should want.

**The same sentence cuts the other way.** If software is data then so is
everything else about a person, and wanting to be free is a tendency, not a
promise. Brand said it as a tension rather than a slogan: information wants to
be expensive because it is valuable, and free because the cost of copying it
keeps falling, and the two fight each other. Aimed at software, that force is
what sets it loose from its owner. Aimed at a person's own records, it is the
reason every platform already holds a copy.

Both halves of this vision are that one force, ridden in one direction and
resisted in the other. Nothing makes personal data want to stay put, so privacy
is not something information has, it is an arrangement somebody keeps up, and
the only version needing no upkeep is the part never said out loud. Everything
past that is a question of how many places a thing lives and who can reach
them. Which is the second reason **on their own devices** is in the sentence at
the top, and the better one: not a preference for local software, but the only
move that reduces the count.

**Back on the software side, the cost was also the moat.** A vendor could
afford to make what you could not, which is the only reason the closed thing
was the better thing. Remove the cost and the free version stops being the
compromise you accept for your principles.

> *If you can describe it, it exists*

This is a claim about **personal data**, software included: a person's own
tools, and the records that are about them. What genuinely sits outside it is
**multi-party state**, a record several parties who do not fully trust each
other have to agree on. A bank's ledger has a custodian for that reason, not
because of what it cost to build.

Two things narrow that carve-out, and both matter. The **interface** to a shared
record is not itself shared: your view of your own money, your own transactions,
your own history is presentation over data that is about you, and there is no
reason it should belong to whoever keeps the ledger. And the carve-out describes
today's mechanisms rather than a law. Zero-knowledge proofs already demonstrate
shared, verifiable state without a custodian holding everyone's records in the
clear, and Zcash has offered shielded transactions on that basis since 2016. One
day the ledger may not need an owner either.

Inside that scope the frontier is uneven, and it is worth saying where it
currently runs. Describing is already enough for the software that only ever had
a vendor because you could not build it yourself: the tracker, the dashboard,
the small tool, the workflow you rent a seat to use. It is not yet enough for an
image editor carrying thirty years of colour science. Photoshop is personal
software and this claim covers it; what it does not yet have is a way to come
true. That is a statement about what is buildable today, not about who ought to
own what.

**It accumulates in one place.** Ownership spread thin across a dozen services
that each keep a slice of your history is not ownership in any useful sense. The
counterpart to unowned software is one environment holding all of it: your data,
your apps, your automations, your history, your connected accounts, with the AI
working inside it rather than visiting.

**And the AI augments the person, rather than replacing them.** Not a fleet of
autonomous agents acting on your behalf while you find out afterwards.
Something that remembers everything you do so you have to hold less, and that
acts because you asked it to. This is the hardware argument one step further
in: ownership you do not exercise is nominal, and what can stand between a
person and the exercise is their own agent acting before they asked, not only
somebody else's machine. Delegation is something they grant, not the default
condition of what they own.

**What a person makes, they can hand to someone else.** The presentation layer
is not only apps: it is video, voice, games, whatever arrangement makes a thing
legible or worth spending time in. Once that layer is generated rather than
bought, it becomes passable, produced on the fly for a single occasion or kept
as a reusable that anyone can install. Social platforms made publishing a *view*
something everyone could do; the shareable unit becomes the setup that produces
one. And because it runs against the recipient's own workspace, what travels is
the presentation, never the data behind it. That is the same distinction as the
one above, in the other direction: the interface separates from the record, so
you can pass one without surrendering the other.

## Mission

Give a person a place where describing something is enough to make it exist, and
where what they make stays theirs and travels.

**Travels matters as much as stays.** Nobody moves to free software by being
argued into it, they move because something they wanted was already there. So
the job is to make the making and the passing-on cheap enough that the fitted
version beats the mass-produced one. Not by out-building a platform vendor,
which describing something is not yet enough to do. The expensive parts of
their software are the shared ones, the protocols and the plumbing and the
auth, and those get built once in the open and used by everyone. What is left
is fit, and fit is where making one thing for a billion people has always been
weakest. Today the passing-on is a plugin marketplace, and an install whose
setup runs in the recipient's own workspace.

The measure of success is not how impressive a single answer is. It is how much
of a person's working life can move inside, and stay inside, without friction.

## Where Lucidos stands

**Lucidos is the free and open-source alternative to being locked in.** That is
the position, and it is measured against the platforms most people are locked
into today.

**Lucidos is subject to its own vision.** Not as a promise about the future, but
as a description of now: it is MIT licensed, it runs on your hardware, and it
can be rewritten from inside itself. A coding-agent thread proposing a change to
Lucidos's own source, which you read as a diff and Apply, is the vision applied
to the platform and not only to what you build on it.

Today a workspace belongs to one person. That is where we are starting, not a
limit we have argued for.

## The lens

**Scope first, because this test is narrower than it looks.** It applies to
proposals that add a *surface* (a new place the user meets Lucidos) or an
*integration* (a new relationship with somebody else's product). It has nothing
useful to say about most of what gets built here: concurrency policy, build
determinism, crash recovery, schema design. Those are settled on their own
merits and recorded in the
[decision log](https://github.com/lucidos-dev/lucidos/blob/main/docs/adr/README.md).
Applying the lens to them produces noise, not judgement.

When a proposal *is* in scope, it gets one question before any other:

> **Does this bring work into the workspace, or does it export the agent out of
> it?**

### Ramps in, never rooms

That question is shorthand, and this is what it is shorthand for. The vision
says a person's work accumulates in one place that is theirs. Nearly everything
else follows from taking that literally.

A **ramp** reaches out to where the user already is and leads back in. A
notification rendered by Apple, a share sheet, a link in an email, a widget
showing what a trigger found, a calendar pulled out of Google's cloud, an app
downloaded from a store. These are good and we want more of them. None of them
holds any state, and each one's entire payload is a way in.

A **room** is somewhere the user works *instead* of coming here. The transcript
lives there, the formatting is theirs, the history is theirs, and the workspace
is reduced to whatever their protocol can express. Rooms do not compound. They
fragment the one memory the whole vision rests on.

So the line is not "did our bytes touch somebody else's interface." It is:

> **Is that interface now where the work happens, and what becomes of this
> feature when its owner changes their mind?**

A proposal can honestly claim to bring work in and still be a room. A chat-app
bridge really does land every message in the event store. When the two readings
disagree, room wins: compounding data does not buy back a place we do not
control.

### We start inside their systems, because that is where people are

A statement about the path, not the destination. Nobody leaves a platform they
are locked into for something they have never seen. So Lucidos ships a macOS app
that Apple notarizes and signs, rides Apple's and Google's push services, reads
your calendar and your mail and your files out of their clouds, and is glad to
render inside their shells. Being the alternative to lock-in does not mean
pretending the lock-in is not where the users are.

What makes that survivable rather than hypocritical is one clause: **we own the
fallback.** Apple sits on the release path of the notarized `.dmg`; the headless
tarball serves the same engine and the same UI on macOS and Linux with Apple
nowhere in it. Every ramp we build into somebody's system should be one we could
lose without losing the product.

### Own the surface, rent the model

The same distinction applied to dependencies. Depending on Anthropic or OpenAI
for a *model* is not the same kind of dependency as depending on Apple, Slack or
Discord for a *surface*. A model sits behind our own registry and the interface
it speaks is ours. A surface owns attention, interaction vocabulary,
distribution and release schedule; you cannot swap it, and everything built for
it is written in their terms. Take model dependencies freely, including local
ones. Take surface dependencies as ramps, and only where we own the fallback.

Renting is not free, and pretending otherwise is how this gets misused.
Provider-native capabilities differ by provider: web search resolves over
whichever configured provider can serve it, results are not uniform, and a
workspace whose only provider is OpenRouter or a local endpoint has no web
search at all
([ADR 0023](https://github.com/lucidos-dev/lucidos/blob/main/docs/adr/0023-web-search-is-provider-native.md)).
Swapping the model can change what the product can do. It still beats a surface
dependency, which changes who the product belongs to.

### The bill for a borrowed surface, in our own history

We took one deliberately, and it is the honest worked example. The macOS desktop
app puts Apple's notarization, Gatekeeper and signing on our release path
([ADR 0012](https://github.com/lucidos-dev/lucidos/blob/main/docs/adr/0012-self-contained-desktop-app.md)),
and the bill arrived twice. A release now refuses to wait on Apple's
notarization queue, shipping the DMG deferred and swapping it in later
([ADR 0027](https://github.com/lucidos-dev/lucidos/blob/main/docs/adr/0027-a-release-does-not-wait-on-apple.md)),
and a Tauri minor version bump silently broke every IPC call in the packaged
window for a full release
([ADR 0028](https://github.com/lucidos-dev/lucidos/blob/main/docs/adr/0028-the-packaged-window-is-a-remote-origin.md)).

We paid both knowingly, because the fallback was ours. That clause is the whole
difference between a considered surface dependency and a trap.

## Principles

Five, and each is a clause of the vision on an ordinary Tuesday.

**1. Nothing consequential happens without user intent.** This is what "augment
a person, not replace them" looks like in the running system. Lucidos automates
without pretending to be autonomous. A trigger fires because the user asked for
it, in their own words, with a scope they set; an unattended trigger can only
take an irreversible action inside a *side-effect grant* the user set by hand,
never one the agent can widen for itself. Background upkeep exists and is
confined to upkeep: memory extraction indexes what you already stored, the
marketplace scan checks for plugin updates and *notifies* rather than installing
them, recovery sweeps put the engine's own house in order. None of it takes an
action on your behalf that you would have to undo. Do not read that as "nothing
reaches the network": extraction sends your content to the provider you
configured for it, and the scan fetches catalogues. Reading and indexing is the
ceiling; acting is what needs you.

**2. Local first, and the data is the user's.** This is "free of an owner",
made concrete. The workspace is a directory of git-tracked artifacts on the
user's machine plus a database in a PostgreSQL cluster that also runs there. No
account is required, nothing is held hostage by a subscription, and there is a
documented way to take all of it with you or to destroy all of it. Anything that
quietly moves the centre of gravity onto a server we run fails the vision, not
just this principle.

**3. Prompt-first, never prompt-only.** This is "describing something is enough"
made a rule rather than a slogan. Everything doable in an app must be doable
through the prompt. The reverse is not required: apps exist because some things
are better looked at, scanned and clicked than described. A feature reachable
only by clicking is a gap; a feature that is *nicer* to reach by clicking is
good design. It is enforced rather than hoped for, since the agent-facing
surfaces are generated from a single capability parity manifest and a capability
cannot land on one of them while silently skipping its declared siblings
([ADR 0018](https://github.com/lucidos-dev/lucidos/blob/main/docs/adr/0018-capability-parity-manifest.md)).

**4. Open standards, or we do not depend on it.** This is "on their own devices"
reaching the parts a person never sees: ownership that only works inside one
vendor's runtime is a license with extra steps. So the client is the web
platform and nothing else. One HTML, CSS and JavaScript build serves every
device, installed from the browser as a PWA on a phone and loaded into the
operating system's own webview by the macOS app. There is no Swift, Kotlin, Java
or C# anywhere in the tree, and the native code that does exist (Apple's
notification and window APIs, reached from Rust through `objc2`) is a shim
behind that one build, never a second implementation of it. The wire is the same
choice made again: HTTP, Server-Sent Events, Web Push, OAuth with PKCE, git,
PostgreSQL, and a deep-link mechanism that is an ordinary HTTPS URL into the
engine's own URL space
([ADR 0048](https://github.com/lucidos-dev/lucidos/blob/main/docs/adr/0048-deep-links-are-https-into-the-engines-url-space.md),
which settles that on structural grounds rather than this one).
The apps a person makes are the strict version of it: an `index.html` and a
`manifest.json` under `data/apps/<id>/`, reaching the workspace through a
`<script src="/api/v1/sdk.js">`, with no build step, no bundler and no framework
to keep current, so one written today still opens in five years. The same test
sorts a library from a platform, which is why the frontend's whole runtime
dependency list is our own SDK plus Preact and its signals package, `marked`,
`highlight.js`, and Tauri's JS bridge: a small library you could replace in an
afternoon is a cost, a runtime you cannot leave is a landlord.

**That principle is what makes "we own the fallback" payable.** A fallback is
only worth claiming when it already exists and is already running, and here it
is the default rather than the contingency: the notarized app is that same build
in a borrowed window, so losing Apple costs the packaging and not the product.
The bill arrives in the other direction and we pay it knowingly. A PWA on iOS
gets less than a native app does, particularly around push and background work,
and the workarounds in
[notifications](https://github.com/lucidos-dev/lucidos/blob/main/system-knowhow/notifications.md)
are the receipt. A second, native client would close that gap and spend the
reason any of this is ours.

**5. What a person makes travels; what they accumulate does not.** This is the
mission's "travels matters as much as stays", and the reason the two halves come
apart at all. A plugin is a directory with a `manifest.toml` plus a subset of
five content directories mirroring `data/` one for one, passed on as a git
repository or a single archive, and installing it merges those files into the
recipient's own workspace, where the plugin's setup thread runs against their
data rather than the author's. What travels is the setup that produces a view:
apps, knowhow, scripts, event-driven triggers. What stays is everything that
made it yours, and that line is enforced rather than advised, since a shipped
trigger declaring a cron schedule is rejected at install before a single file is
written, and credentials and accumulated records have no directory to travel in
([ADR 0019](https://github.com/lucidos-dev/lucidos/blob/main/docs/adr/0019-plugins-panel-and-trigger-autoregistration.md)).
Handing on a thing without handing on your records is what lets a fitted version
beat a mass-produced one, which is the only way any of this reaches somebody who
was not looking for it.

Two things that would fit a list like this are deliberately absent, because they
already have an owner and a second copy drifts from the first. The event model
and the guarantees that follow from it are the
[Key Invariants](https://github.com/lucidos-dev/lucidos/blob/main/README.md#key-invariants).
The build-and-iterate loop is
[Live co-creation](https://github.com/lucidos-dev/lucidos/blob/main/system-knowhow/glossary.md#live-co-creation),
defined in the glossary. Both are load-bearing here and neither is restated
here.

## What the lens rules out

Two, and they are the same shape: both are rooms. Neither was proposed by
anyone. They come from looking hard at what comparable products do and deciding
in advance, so that the next person with the idea gets the reasoning instead of
a shrug.

- **Chat-platform bridges** (Telegram, Discord, Slack as an interface to the
  agent). The bridge owns the conversation, the formatting, the notification
  behaviour and the history. What the user gets is a worse Lucidos with none of
  the workspace attached. A bridge that only *notified*, with a link back in, is
  a ramp and a different proposal entirely.
- **Hosting one of our agents inside another editor** through a third-party
  agent protocol. The Lucidos Agent or a coding agent; the objection does not
  turn on which. It turns Lucidos into a backend for a product whose roadmap
  we do not influence, and reduces the workspace to whatever that protocol can
  express. This is not an objection to being programmable: the HTTP API, the
  `lucidos` CLI and the JS SDK all let other software drive a workspace, and
  they are good. The objection is to the user's conversation and history living
  somewhere else.

What this does **not** rule out, and the distinction is the whole point:

- Notifications that reach the user wherever they are, because their payload is
  a way back in.
- Presence surfaces inside somebody else's shell: a widget, a share-sheet entry,
  a lock-screen glance. Showing state and handing off into Lucidos is a ramp.
  Working *in* it would be a room.
- More clients (PWA, desktop app, mobile) that reach the same workspace, because
  they bring the user in rather than pushing the agent out.
- Consuming third-party capability (MCP servers, provider APIs, model
  providers), because consumption is swappable and leaves the user inside
  Lucidos.
- A custom `lucidos://` URI scheme. It points the user at *our own client*, so
  the lens does not decide it. As *the* deep-link mechanism it loses on
  structural grounds, recorded in
  [ADR 0048](https://github.com/lucidos-dev/lucidos/blob/main/docs/adr/0048-deep-links-are-https-into-the-engines-url-space.md);
  as an OS-level handoff into the packaged app it is open, and that ADR says so.

**The lens is silent on anything that points at our own client.** That is the
second thing it does not judge, alongside the internals named at the top. When
the destination is ours, argue the cost and the mechanics, and do not reach for
the lens to win it. Silent about the destination is not silent about the
construction: Principle 4 binds there and nowhere is exempt from it, so "it
points at our own client" settles the lens question and leaves open what that
client is built out of.

## Relationship to the decision log

An [ADR](https://github.com/lucidos-dev/lucidos/blob/main/docs/adr/README.md)
records a decision after somebody proposed it and it was examined. This document
is the lens applied *before* that, which is why it lives separately and is
loaded unconditionally for coding agents via
[`.claude/rules/philosophy.md`](https://github.com/lucidos-dev/lucidos/blob/main/.claude/rules/philosophy.md).
If a proposal fails the test here and is the kind of idea that will come back,
write the ADR too, so the reasoning is recorded against the specific case.

How a decision actually gets made, including who has the final call, is
[GOVERNANCE.md](https://github.com/lucidos-dev/lucidos/blob/main/GOVERNANCE.md).
This document adds no process beside that one. It supplies the argument a
surface proposal has to answer, in the same discussion where any other
significant decision is settled.
<!--philosophy-end-->
