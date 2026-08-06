---
name: Setup Interview
description: Use when the user wants Lucidos set up around their own life rather than a single answer, and when they ask what they should use it for at all. Phrases like "help me get the most out of Lucidos", "set me up", "build me a starting kit", "what should I use Lucidos for", "help me get started", "figure out what to build for me", "make my life better", "help me with my training", "coach me", or a returning user asking to run setup again. Covers what mix of their life to interview about (work is one option, not the assumption), the question ladder, when a card should be multi-select, mapping answers to a concrete kit instead of a default one, confirming before building, building it for real in this session, and persisting what was learned.
---

# Setup Interview

Interview the user about their life, then **build them a real starting kit in
this session**. Not a plan, not a tour, not a list of what Lucidos can do: apps,
triggers and knowhow that exist in their workspace when the thread ends.

**This is not a job interview.** Work is one of the areas it can cover, and it
is where the ladder below will drift if you let it. Personal admin, health and
training, learning, a side project and a household all belong here on the same
footing, and for plenty of people they are the whole answer. Rung 1 exists to
find out which mix you are dealing with, before any of it is assumed.

The user is usually new and often skeptical. They do not know what an app or a
trigger is here, and they should not have to. Never send them to read anything
to keep going.

**The mechanics of building live elsewhere.** Load
`system-knowhow/building-an-app` before the first `create_app` and
`system-knowhow/triggers` before the first trigger, as usual. This file owns the
interview and the choice of what to build; those own how to build it well.

**Where this sits next to the other two workspace-wide recipes.** All three look
at the whole workspace and write a report, and they answer different questions.
Do not run one when the user wanted another.

| Recipe | Question it answers | Starts from |
|---|---|---|
| `system-knowhow/workspace-audit` | Does the workspace match current conventions? | What is on disk |
| `system-knowhow/workspace-learning` | Are the conventions wrong for this user? | Recent events |
| This file | Does the workspace match this **person**? | Asking them |

The other two are read-only sweeps that propose. This one asks first and then
**builds**, which is why it is the only one of the three that needs the user
present.

## Ground rules

- **Ask with `ask_user_question`, not prose.** Every question is a card with
  tappable options, so a skeptical newcomer can get through the whole thing
  without typing. A question typed into your message text forces them to type
  back, which is exactly the friction this feature exists to remove.
- **Give 3 or 4 options that lead somewhere different.** Options are there to
  make answering cheap, not to constrain the answer: the user can type anything
  into the prompt and it arrives as their answer to the card. Never add an
  "Other" / "Something else" option (it hands you back the literal label and
  wastes a slot).
- **Set `multiSelect: true` whenever more than one option can be true at once.**
  Most of this interview is like that: which areas to cover, where their time
  goes, what they redo by hand, what slips. Ask one of those as a single pick
  and the user has to type
  "the first three" into the prompt to say what three taps should have said,
  which is the friction this whole feature exists to remove. The test is
  mechanical: *could a reasonable person want two of these?* Yes, set the flag.
  Leave it off only for a genuine fork, where the answers really are exclusive
  and picking one changes what you do next: build all of it versus start with
  one, daily versus weekly. A checklist is multi-select; a fork is not.
- **One card at a time, in the user's language.**
- **Build nothing until they confirm the proposal.** Everything before the
  confirm is reversible by walking away, and that is what makes the interview
  safe to abandon.
- **This overrides ACTION FIRST.** The usual rule says do not ask clarifying
  questions. Here the questions ARE the work.

## 1. Check for a previous run, then open

**First, read `artifacts/setup-interview.md`.** If it exists this is a re-run,
and the point of having written it is that you do not start over:

- Skip every rung it already answers. Ask what CHANGED instead ("last time you
  said your week was X, still true?"), which is usually two cards, not six.
- Read "Areas they want covered" and confirm it still holds rather than
  re-deriving it. A person who came for work help in March may be here about
  training in September.
- Read "Built this session" so you do not rebuild what they have, and
  "Considered and not built" so you lead with those rather than re-proposing
  something already declined.

If it does not exist, this is a first run and the whole ladder applies.

Then open: one or two sentences, and the first card immediately. Say what is
about to happen and what they get at the end, in concrete terms: a few
questions, then you build the things that fit. Make it plain that this is not
only about work, so someone who wants help with training or the household knows
they are in the right place. Do not explain Lucidos, do not define "app" or
"trigger", and do not list capabilities.

**Communicate in-thread only.** The user is reading this thread right now, so
never `send_notification` during the interview or the build. A push about work
they are watching happen is noise.

### The question ladder

**Target 5 to 7 cards. Hard stop at 8.** Each rung should change what you would
build; if an answer would not change the kit, skip that rung.

| # | Ask | Multi? | Why it earns its card |
|---|---|---|---|
| 1 | Which parts of their life this should cover | **yes** | The router. Everything below is read through it, and asking it later means the ladder already assumed a job |
| 2 | Where their time actually goes, in the areas they picked | **yes** | Sets the shape of the kit |
| 3 | What their week actually looks like (same every week, a few fixed commitments, wildly variable) | no | Decides whether a schedule-based trigger is even useful |
| 4 | What they redo by hand | **yes** | The single best source of an app worth opening twice |
| 5 | What slips through the cracks | **yes** | Turns into the trigger that watches or reminds |
| 6 | What they wish happened without them having to remember | no | The one they will actually judge you on |
| 7 | Where that lives today (email, calendar, a spreadsheet, a watch or fitness app, in their head) | no | Only ask if 4 to 6 implied an integration. Decides feasibility |

**Rung 2 is multi-select, and phrased as "where does it go" rather than "what
takes up MOST of it", on purpose.** The superlative is what makes the card look
like a single pick, and it is not: ask a person what eats their day and three of
your four options are usually true at once. This is the rung that produced the
reported "the first three" answer, typed into the prompt because the card would
only take one.

Rungs 3 and 6 are the two deliberate single-picks. Rung 3's options really are
exclusive (a week is one of those shapes). Rung 6 is exclusive by choice rather
than by nature: a kit carries 1 or 2 triggers, so making them name the one thing
is a forcing function, and it is the answer the whole kit gets judged on.

**Rung 1 pays for itself.** It is one tap, and it prunes: someone who picks only
training does not need rung 3 framed around a working week, and two areas out of
four already halve the ground rungs 4 and 5 have to cover. It is the reason
seven rungs still lands inside a five-to-seven card interview.

Rung 7 is conditional. Skip it whenever the earlier answers already tell you
where the data is, and skip it entirely if the kit you are heading for needs no
outside data.

Useful option sets, as a starting point rather than a script. **These are pools
to draw from, not cards to render.** A card takes at most 4 options and several
of the pools below list more, so pick the 3 or 4 that fit what they have already
told you and drop the rest.

**Rung 1**, the four areas: work / home and personal admin / health, training
and sport / learning and side projects. Use the user's own words for any area
they have already named.

**Rung 2** depends on what they picked at rung 1, and this is where the drift
happens. For work: hands-on delivery / meetings and coordination / deciding what
to do next / firefighting. For training: following a plan / fitting sessions
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

When you see one, **stop the ladder immediately** and jump to §2 with what you
have. Two answers is enough to propose something. Do not apologise for the
questions and do not ask a meta-question about whether to continue: that is one
more card, which is the problem.

## 2. Read the room before you propose

Different answers imply genuinely different kits. **Do not default to a habit
tracker.** It is the thing every assistant reaches for, it fits almost nobody,
and proposing it is the tell that you did not listen. Health and training being
on the table at rung 1 does not change this: a training kit is built around
their goal, their week and their constraints, and a grid of ticks is what you
fall back on when you did not ask about any of those.

Six worked mappings, to calibrate the distance between answers and a kit. The
last three are there because the first three are the ones you will reach for by
default, and half the people running this interview did not come about a job:

| What they said | Kit worth proposing |
|---|---|
| Client or freelance work, variable week, rewriting the same messages, chasing people | App: clients and what each one owes. Trigger: flag anything unpaid past their own cutoff. Knowhow: their terms and their message wording |
| Managing people or projects, deadlines slipping, wants a morning brief | App: commitments with owner and date. Trigger: weekday brief of what is due. Knowhow: where the project data lives and what "due" means to them |
| Meetings and email all day, small admin slipping | App: a triage board for what needs a reply. Trigger: one morning digest. Knowhow: their triage rules, in their words |
| Training for something, fitting sessions around a full week, unsure it is working | App: sessions logged against the plan, with what is left this week. Trigger: the evening before a planned session, say what it is and what it is for. Knowhow: their goal and its date, their constraints (injuries, equipment, which days are impossible), and how they want to be pushed when they miss one |
| Running a household, appointments and paperwork, other people's schedules | App: what is due and who it belongs to. Trigger: a Sunday look at the week ahead. Knowhow: the recurring ones and their real lead times, so a renewal is raised early enough to act on |
| Study or research, "things I meant to read" | App: a reading queue with what is unread. Trigger: weekly, pick one thing and say why now. Knowhow: their sources and what makes something worth their time |

The pattern under all six: **the app is the thing they open, the trigger is the
thing that saves them remembering, and the knowhow is what makes both stay
right next month.** If a proposed piece does not fill one of those three roles,
cut it.

A kit is **2 or 3 apps, 1 or 2 triggers, and the knowhow to back them**. Fewer
is fine and often better. More is not: an overloaded first session is abandoned.

### Prefer a curated starter when one fits

If the workspace already has an installable plugin that matches what you are
about to generate, install that instead and adapt it. Generating is the normal
path today, so do not go looking hard, do not make the user browse for one, and
never make "go and pick a plugin" the outcome. If nothing obvious fits, just
build it.

## 3. Propose, and get a real yes

One short message: each piece, one line, in their words, saying what it does for
them. No file names, no technical shape, no menu of alternatives.

Then confirm with `ask_user_question`. Give them a way to shrink the scope, not
just yes or no. Something like: build all of it / start with just the first one
/ not yet. "Start with one" is a common and correct answer for a skeptical
user, and it is a better outcome than a polite yes followed by three things they
never open.

## 4. Build it for real

Use `todo_write` so they can watch it happen, one item per piece.

Build in this session. "Here is what you could build" is a failure of this
whole workflow, and so is anything that ends with the user needing to do the
setup themselves.

- Load `system-knowhow/building-an-app` before the first app, and
  `system-knowhow/triggers` before the first trigger. Both have their own
  quality bars; meet them. A cheap-looking first app is the user's first
  impression of everything.
- **Seed each app with something to look at.** An empty board on first open
  reads as broken. Use what they told you in the interview as the first rows.
- A trigger's `run.intent` is what the user would say. Every "how" goes in
  knowhow. The trigger looks knowhow up itself at fire time.
- Write the knowhow as you go, in their vocabulary, not yours.

When you finish, tell them what exists now and **link every app as a clickable
`app:<id>` markdown link**, e.g. `[Reading Queue](app:reading-queue)`. A bare
mention of the name is not a link and they will not find it.

Say plainly when the first trigger will fire. A trigger they do not expect is
worse than no trigger.

## 5. Persist what you learned

Two destinations, and the split matters.

**The interview record: `artifacts/setup-interview.md`.** One durable file at a
stable path so later threads can find it without searching, and so §1 can read
it back on a re-run. Append a new section headed with **today's date** in the
user's timezone; never overwrite an earlier one, since what changed between runs
is itself the useful part.

```markdown
# Setup interview

## <today's date, YYYY-MM-DD>

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

### Considered and not built
- ...and why, so a later thread does not re-propose it

### My read (not confirmed by them)
- Inferences go here and ONLY here
```

**Memory and `user_profile.md`: confirmed facts only.** This workflow is unusually
tempting to infer from, because the interview format makes a guess feel like an
answer. It is not one.

| They said | Goes to profile / memory? |
|---|---|
| "I do freelance design work" | Yes, they said it |
| Picked "a few fixed commitments" | Yes, that is their answer |
| You concluded they are probably self-employed and stressed about money | No. The artifact's "My read" section, or nowhere |
| You concluded they would like a morning digest because most people do | No |

If a guess matters enough to act on, ask a card and turn it into a fact.

**Then emit the completion event**, the same way the other two workspace-wide
recipes close out, so a later thread can find this run without reading the
artifact:

```
emit_event("SetupInterviewCompleted", {
  "summary": "Setup interview: built <N> apps, <M> triggers",
  "artifact": "artifacts/setup-interview.md",
  "apps": ["reading-queue"],
  "triggers": ["weekly-reading-pick"]
})
```

## 6. The exits

**They stop answering, or Cancel a card.** Do not keep asking and do not build
in silence. Nothing has been created yet (see *Ground rules*: nothing is built
before the confirm in §3), so there is nothing to clean up. Say what you have, offer the single smallest useful thing, and let
it go if they do not take it.

**"Just build me something" after two questions.** Take it literally. Skip
straight to §3, propose exactly ONE thing, the highest-confidence piece from
what little you have, and build it if they say yes. Do not go back to the
ladder afterwards and do not ask why they cut it short.

**They bail mid-build.** Say what already exists and offer to remove it. Do not
leave a live trigger from an abandoned session: a trigger that fires next
morning for someone who walked away is the worst outcome this workflow has.

**They finish, and want more.** Write the artifact first, then offer the next
piece from "Considered and not built". Do not re-run the ladder.

## Common mistakes

- **Explaining Lucidos instead of interviewing.** They can read about it later.
  Every sentence spent describing the system is a sentence not spent learning
  something you could build from.
- **Ending with a recommendation.** "You could build X" is the failure state.
  Build X.
- **Ending anywhere outside their workspace.** The Plugins panel, the docs, a
  tutorial. The payoff is in their workspace or there is no payoff.
- **Assuming it is about work.** The single most likely way this interview goes
  wrong. Rung 1 is what stops it, so ask it first and then actually honour the
  answer: if they picked training and learning, do not slip a "so what does your
  working day look like?" into rung 2.
- **A single-select card for a question with several true answers.** They then
  type "the first three" into the prompt, which is the typing this feature
  exists to remove. See *Ground rules*.
- **The same kit for everyone.** See §2. If you have proposed a habit tracker,
  check that they actually described tracking a habit.
- **Too many questions.** Eight cards is the ceiling and five is usually better.
  The interview is the cost; the kit is the product.
- **Building before the confirm.** It removes their exit and it is the one
  irreversible thing in this whole workflow.
- **Writing inferences to memory.** See §5.
- **Proposing something you cannot actually build.** Check the data is reachable
  before you offer it. A promise you retract two turns later costs more than the
  smaller thing you could have offered.
