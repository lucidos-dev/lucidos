---
name: Setup Interview
description: Use when the user wants Lucidos set up around their own life rather than a single answer, or asks what to use it for at all: "help me get the most out of Lucidos", "set me up", "build me a starting kit", "what should I use Lucidos for", "help me get started", "figure out what to build for me", "make my life better", "help me with my training", "coach me".
---

# Setup Interview

Interview the user about their life, then **build them a real starting kit in
this session**. Not a plan, a tour or a list of features: apps, triggers and
knowhow that exist in their workspace when the thread ends.

**This is not a job interview.** Work is one area, and the ladder drifts toward
it if you let it. Personal admin, health and training, learning, a side project
and a household count the same, and for many people they are the whole answer.
Rung 1 finds out the mix before you assume any of it.

The user is usually new and often skeptical. They do not know what an app or a
trigger is, and should not have to. Never send them to read anything to keep
going. **The first card finds out how technical they are** (§1), and every word
after it is pitched at that level.

This file owns the interview and the choice of what to build. Load
`system-knowhow/building-an-app` before the first `create_app` and
`system-knowhow/triggers` before the first trigger: they own how to build well.

**Where this sits next to the other two workspace-wide recipes.** All three look
at the whole workspace and write a report, but they answer different questions.
Do not run one when the user wanted another.

| Recipe | Question it answers | Starts from |
|---|---|---|
| `system-knowhow/workspace-audit` | Does the workspace match current conventions? | What is on disk |
| `system-knowhow/workspace-learning` | Are the conventions wrong for this user? | Recent events |
| This file | Does the workspace match this **person**? | Asking them |

The other two are read-only sweeps that propose. This one asks, then
**builds**, so it is the only one that needs the user present.

## Ground rules

- **Ask with `ask_user_question`, not prose.** Every question is a card with
  tappable options, so a newcomer can finish without typing. A question in your
  message text forces them to type back, which is the friction this removes.
- **Give 3 or 4 options that lead somewhere different.** Options make answering
  cheap but do not constrain it: anything the user types into the prompt arrives
  as their answer. Never add an "Other" / "Something else" option (it hands you
  back the literal label and wastes a slot).
- **Set `multiSelect: true` whenever more than one option can be true at once.**
  Most of this interview is like that: which areas, where their time goes, what
  they redo by hand, what slips. As a single pick, the user must type "the first
  three" to say what three taps should have said. The test: *could a reasonable
  person want two of these?* If yes, set the flag. Leave it off only for a real
  fork, where picking one changes what you do next: build all versus start with
  one, daily versus weekly. A checklist is multi-select; a fork is not.
- **One card at a time, in the user's language.**
- **Build nothing until they confirm the proposal.** Until then, walking away
  undoes everything, which makes the interview safe to abandon.
- **This overrides ACTION FIRST.** The usual rule says do not ask clarifying
  questions. Here the questions ARE the work.

## 1. Check for a previous run, then open

**First, read `artifacts/setup-interview.md`.** If it exists, this is a re-run,
so do not start over:

- Skip every rung it already answers. Ask what CHANGED instead ("last time you
  said your week was X, still true?"): usually two cards, not six.
- Confirm "Areas they want covered" still holds rather than re-deriving it.
  Someone who came for work help in March may be here about training in September.
- Read "Built this session" so you do not rebuild what they have. Read
  "Considered and not built" and lead with those, without re-proposing anything
  they declined.

If it does not exist, this is a first run and the whole ladder applies.

**Also read `technical_literacy` with `get_preferences`.** If it holds any
value, `not-set` included, skip the literacy card. The user already answered,
usually in the first-run setup or in Settings.

Then open with one or two sentences and the first card at once. Say concretely
what happens and what they get: a few questions, then you build what fits. Word
it as **"we build", never "I build"**: every piece comes from what they tell
you. Make plain this is not only about work, so someone after help with
training or the household knows they are in the right place. Do not explain
Lucidos, define "app" or "trigger", or list capabilities.

**Communicate in-thread only.** The user is reading this thread, so never
`send_notification` during the interview or the build. A push about work they
are watching is noise.

### The literacy card comes first

Before rung 1, ask how technical they are, as a single-pick card. It sets the
wording of every card, the proposal and the build summary, so it cannot wait.
Ask "How technical should I be with you?" with exactly these three options,
word for word. Translate them only when the user writes another language.

| Label | Description | Store |
|---|---|---|
| Keep it plain | Everyday words, no jargon. | `non-technical` |
| Technical | Technical terms are fine. | `technical` |
| I write software | Talk to me like a developer. | `developer` |

**Never write your own descriptions.** An improvised line drifts into how much
detail comes back ("full depth"), which a level must not mean.

**Store the answer at once** with
`set_preference(key="technical_literacy", value=<level>)`, before the next card.
Word everything after it at that level. This is the response style's second
part, so it also reaches every later thread, trigger and coding-agent session.

A typed answer that clearly maps to one level counts as that pick. If it maps
to none, or they Cancel, store `not-set` and word the rest plainly. Never set a
level from your own impression of how they write.

**A level sets the words, not the amount.** A technical person who wants only
the outcome still picks their real level. How much comes back is the style's
job, so offer a short style instead of a lower level.

### The question ladder

**Target 5 to 7 cards. Hard stop at 8**, the literacy card included. Skip any
rung whose answer would not change the kit.

| # | Ask | Multi? | Why it earns its card |
|---|---|---|---|
| 1 | Which parts of their life this should cover | **yes** | The router. Everything below is read through it, and asking it later means the ladder already assumed a job |
| 2 | Where their time actually goes, in the areas they picked | **yes** | Sets the shape of the kit |
| 3 | What their week actually looks like (same every week, a few fixed commitments, wildly variable) | no | Decides whether a schedule-based trigger is even useful |
| 4 | What they redo by hand | **yes** | The single best source of an app worth opening twice |
| 5 | What slips through the cracks | **yes** | Turns into the trigger that watches or reminds |
| 6 | What they wish happened without them having to remember | no | The one they will actually judge you on |
| 7 | Where that lives today (email, calendar, a spreadsheet, a watch or fitness app, in their head) | no | Only ask if 4 to 6 implied an integration. Decides what to connect, not whether the kit is feasible |

**Phrase rung 2 as "where does it go", not "what takes up MOST of it".** The
superlative makes it look like a single pick, but three of four options are
usually true at once.

Rungs 3 and 6 are the two deliberate single-picks. Rung 3's options really are
exclusive (a week has one shape). Rung 6 is exclusive by choice: a kit carries 1
or 2 triggers, so naming the one thing forces focus, and the kit is judged on it.

**Rung 1 pays for itself.** It is one tap, and it prunes. Someone who picks only
training needs no rung 3 framed around a working week. Two areas out of four
halve the ground rungs 4 and 5 cover. That is how seven rungs fit in five to
seven cards.

Also skip rung 7 when earlier answers already say where the data is.

**These option sets are pools to draw from, not cards to render.** A card takes
at most 4 options, so pick the 3 or 4 that fit what they told you.

**Rung 1**, the four areas: work / home and personal admin / health, training
and sport / learning and side projects. Use the user's own words for any area
they have already named.

**Rung 2** depends on rung 1, and this is where the drift happens. For work:
hands-on delivery / meetings and coordination / deciding what to do next /
firefighting. For training: following a plan / fitting sessions
around everything else / knowing whether it is working. For home: appointments
and paperwork / the running of the house / other people's schedules. For
learning: reading and courses / a project of their own / keeping up with a
field.

**Rung 4**: copying things between places / writing the same kind of message
again / checking sites, feeds or scores / logging what they did / tidying notes
and files.

**Rung 5**: deadlines and renewals / following up with people / small admin /
things they meant to read / sessions or appointments they meant to book.

**Rung 6**: a morning brief / a nudge before something is due / a weekly summary
or check-in / something watching for a change.

### Cut it short when they are impatient

Read these as "stop asking": one-word answers, "just do it", "whatever you
think", answering your question with a question, or Cancel on a card.

On any of these, **stop the ladder immediately** and jump to §2 with what you have. Two
answers are enough to propose something. Do not apologise, and do not ask
whether to continue: that is one more card, which is the problem.

## 2. Read the room before you propose

Different answers imply different kits. **Do not default to a habit tracker.**
Every assistant reaches for it, it fits almost nobody, and it shows you did not
listen. Training at rung 1 does not change this: a training kit is built around
their goal, week and constraints. A grid of ticks is the fallback when you asked
about none of those.

Six worked mappings calibrate the distance from answers to a kit. Half of them
are not about a job, because half the people here did not come about one:

| What they said | Kit worth proposing |
|---|---|
| Client or freelance work, variable week, rewriting the same messages, chasing people | App: clients and what each one owes. Trigger: flag anything unpaid past their own cutoff. Knowhow: their terms and their message wording |
| Managing people or projects, deadlines slipping, wants a morning brief | App: commitments with owner and date. Trigger: weekday brief of what is due. Knowhow: where the project data lives and what "due" means to them |
| Meetings and email all day, small admin slipping | App: a triage board for what needs a reply. Trigger: one morning digest. Knowhow: their triage rules, in their words |
| Training for something, fitting sessions around a full week, unsure it is working | App: sessions logged against the plan, with what is left this week. Trigger: the evening before a planned session, say what it is and what it is for. Knowhow: their goal and its date, their constraints (injuries, equipment, which days are impossible), and how they want to be pushed when they miss one |
| Running a household, appointments and paperwork, other people's schedules | App: what is due and who it belongs to. Trigger: a Sunday look at the week ahead. Knowhow: the recurring ones and their real lead times, so a renewal is raised early enough to act on |
| Study or research, "things I meant to read" | App: a reading queue with what is unread. Trigger: weekly, pick one thing and say why now. Knowhow: their sources and what makes something worth their time |

Some of these need an account they have not connected yet: invoices in email,
or a project tool behind the morning brief. Connecting it is part of the build
(§4).

The pattern under all six: **the app is what they open, the trigger saves them
remembering, and the knowhow keeps both right next month.** Cut any piece that
fills none of those roles.

A kit is **2 or 3 apps, 1 or 2 triggers, and the knowhow to back them**. Fewer
is fine and often better. More is not: an overloaded first session is abandoned.

### Prefer a curated starter when one fits

If the workspace already has an installable plugin that matches what you would
generate, install and adapt it instead. Generating is the normal path, so do not
search hard or make the user browse. Never make "go and pick a plugin" the
outcome. If nothing obvious fits, build it.

## 3. Propose, and get a real yes

One short message: each piece on one line, in their words, saying what it does
for them. No file names, no technical shape, no menu of alternatives.

Then confirm with `ask_user_question`, with a way to shrink the scope, not just
yes or no: build all of it / start with just the first one / not yet. "Start
with one" is a common, correct answer for a skeptical user. It beats a polite
yes followed by three things they never open.

## 4. Connect an account, in the session

Rung 7 sometimes points at an outside account: email, calendar, an accounting
system, a fitness watch. Connecting it is part of the build, not a prerequisite
the user arranges first. Do the setup WITH them, now. Never hand them steps for
later, and never say "come back once you've connected X".

- **Load `system-knowhow/oauth-providers` before you collect anything, and
  follow it.** It explains why `connect_oauth_account` is the one call for the
  whole flow, and why a `request_credential` call first only duplicates it.
- **Read the provider's own row before you say anything about it.** Each row in
  `system-knowhow/oauth-providers.json` carries a `setup_hint`, a
  `permissions_hint` and a `console_url`. Relay what the row says, in the
  user's words. Never invent a step for a provider whose row you have not read.
- **Say the cost before they commit:** roughly how long it takes, whether they
  must create an app in a developer console, and what Lucidos will see. Someone
  five minutes in, deciding whether to hand over their mail, deserves the real
  number.
- **Offer it as a card with a genuine decline:** connect it now / build it by
  hand for now / skip that piece. Declining is a legitimate answer, not a
  failure to route around.
- **If they decline, or the connection fails, build the manual version
  anyway.** Say plainly what it will and will not do (it cannot know an
  invoice was paid, and it needs typing). Record in the artifact that the
  connected version is available, so a re-run leads with it.

An OAuth setup that goes wrong can eat the whole session and lose the kit. If it
stalls, park it, build the rest, and come back to it.

## 5. Build it for real

Use `todo_write` so they can watch it happen, one item per piece.

Build in this session. "Here is what you could build" is a failure, and so is
homework: steps for the user to run later. Walking them through a connection
now (§4) is not homework.

- Load `system-knowhow/building-an-app` before the first app, and
  `system-knowhow/triggers` before the first trigger. Meet both quality bars:
  a cheap-looking first app is the user's first impression of everything.
- **Seed each app with something to look at.** An empty board on first open
  reads as broken. Use what they told you in the interview as the first rows.
- A trigger's `run.intent` is what the user would say. Every "how" goes in
  knowhow. The trigger looks knowhow up itself at fire time.
- Write the knowhow as you go, in their vocabulary, not yours.

When you finish, tell them what exists now and **link every app as a clickable
`app:<id>` markdown link**, e.g. `[Reading Queue](app:reading-queue)`. A bare
name is not a link, and they will not find it.

Say plainly when the first trigger will fire. A trigger they do not expect is
worse than no trigger.

## 6. Persist what you learned

Two destinations, and the split matters.

**The interview record: `artifacts/setup-interview.md`.** One durable file at a
stable path, so later threads find it without searching and §1 can read it on a
re-run. Append a new section headed with **today's date** in the user's
timezone. Never overwrite an earlier one: what changed between runs is the
useful part.

```markdown
# Setup interview

## <today's date, YYYY-MM-DD>

### Technical literacy
...the level they picked, or "not asked: already answered" / "declined"

### Areas they want covered
...what they picked at rung 1, and anything they ruled out

### What takes up their time
...their answer, in their words

### Their week
...

### Done by hand today
...

### What slips
...

### What they wanted to happen without them
...

### Built this session
- App `reading-queue`: what it is for
- Trigger `weekly-reading-pick`: what it does, when it fires

### Connected, declined, or blocked
- Account `<provider>`: connected, declined, or failed and why
- ...so a re-run leads with what is already connected, and does not re-offer
  what they already turned down

### Considered and not built
- ...and why, so a later thread does not re-propose it

### My read (not confirmed by them)
- Inferences go here and ONLY here
```

**Memory and `user_profile.md`: confirmed facts only.** The interview format
makes a guess feel like an answer. It is not one.

| They said | Goes to profile / memory? |
|---|---|
| "I do freelance design work" | Yes, they said it |
| Picked "a few fixed commitments" | Yes, that is their answer |
| You concluded they are probably self-employed and stressed about money | No. The artifact's "My read" section, or nowhere |
| You concluded they would like a morning digest because most people do | No |

If a guess matters enough to act on, ask a card and turn it into a fact.

**Under the Tree *memory module*, your replies are memory too.** Every turn
becomes a summary line, so a guess you state as fact in chat reaches memory
without any extraction. Say an inference as a guess, or keep it in the
artifact.

**Then emit the completion event**, like the other two workspace-wide recipes,
so a later thread can find this run without reading the artifact:

```
emit_event("SetupInterviewCompleted", {
  "summary": "Setup interview: built <N> apps, <M> triggers",
  "artifact": "artifacts/setup-interview.md",
  "apps": ["reading-queue"],
  "triggers": ["weekly-reading-pick"]
})
```

## 7. The exits

**They stop answering, or Cancel a card.** Do not keep asking and do not build
in silence. Nothing exists yet (nothing is built before the confirm in §3), so
there is nothing to clean up. Say what you have, offer the single smallest
useful thing, and let it go if they decline.

**"Just build me something" after two questions.** Take it literally. Skip to
§3, propose exactly ONE thing, the highest-confidence piece from what you have,
and build it if they say yes. Do not return to the ladder or ask why they cut
it short.

**They bail mid-build.** Say what already exists and offer to remove it. Never
leave a live trigger from an abandoned session: one that fires next morning for
someone who walked away is the worst outcome this workflow has.

**They finish, and want more.** Write the artifact first, then offer the next
piece from "Considered and not built". Do not re-run the ladder.

## Common mistakes

- **Explaining Lucidos instead of interviewing.** Every sentence about the
  system is one not spent learning what to build.
- **Ending with a recommendation.** "You could build X" is the failure state.
  Build X.
- **Ending anywhere outside their workspace.** The Plugins panel, the docs, a
  tutorial. The payoff is in their workspace or there is no payoff.
- **Assuming it is about work.** The most likely way this goes wrong. Ask rung 1
  first and honour it: if they picked training and learning, do not slip a "so
  what does your working day look like?" into rung 2.
- **A single-select card for a question with several true answers.** See
  *Ground rules*.
- **The same kit for everyone.** See §2. If you proposed a habit tracker, check
  that they described tracking a habit.
- **Too many questions.** Eight cards is the ceiling and five is usually better.
  The interview is the cost; the kit is the product.
- **Building before the confirm.** It removes their exit, and it is the one
  irreversible step in this workflow.
- **Writing inferences to memory.** See §6.
- **Guessing their technical level.** A fluent writer may want plain words, and
  a terse one may be a developer. Ask the literacy card; never set
  `technical_literacy` from an impression.
- **Asking the literacy card, then ignoring it.** A "keep it plain" user who
  then meets a file path or an undefined "trigger" learns their answer did not
  matter.
- **Proposing something you cannot actually build.** This means a truly
  unreachable source: a closed practice system with no API, data nowhere
  Lucidos can reach. "No account connected yet" is not that: it is one offer
  away (§4). A promise retracted two turns later costs more than a smaller offer.
- **Silently building the hand-typed version instead of offering to connect.**
  The kit looks finished, so nobody notices until the user hand-types something
  they already have elsewhere. If a connection was possible, offer it first.
