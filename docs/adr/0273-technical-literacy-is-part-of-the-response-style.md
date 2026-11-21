# 0273: Technical literacy is the response style's second part, stated never inferred, and reaches coding agents

- **Status**: Accepted
- **Date**: 2026-09-24

## Context

The maintainer asked for the setup interview to establish how technical the user
is, and for every agent to use that in all communication. Nothing modelled it.
The interview never asked, and its knowhow simply assumed "the user is usually
new". Coding-agent sessions see no user profile at all.

ADR 0216 had just made the *response style* a workspace preference in the
cached system tier. It explicitly left coding agents untouched.

## Decision

Technical literacy is the response style's **second part**, beside the style
(the shape of an answer). It is one global preference, `technical_literacy`,
holding one of three levels. Unset adds nothing.

1. Chat and trigger threads get it inside the existing `RESPONSE STYLE:`
   section, joined with the style at render time.
2. Coding-agent sessions get it too, read once at spawn. This amends ADR 0216
   for this part only: the style still does not reach them.
3. A voice call's talker gets it in its instructions, read at call start.
4. Only a level the user stated or picked is stored, never one inferred.
5. It is a mandatory preference. The first chat turn asks for it, and
   `not-set` records a decline. Only a turn a person sent from a device enters
   setup mode, so triggers, sub-threads and tasks never stop on it.
6. A level sets which words need no explanation, never how much to say.
7. At the lowest level, the level also decides which questions reach the
   user: never one they cannot answer. In chat it also steers them away from
   coding agents, without hiding them.
8. The style widens to "the shape of an answer". Lucidos ships Standard,
   Concise, Minimal (the outcome, not the process) and Learning.

## Rationale

**Part of the response style, not its own setting.** Length and vocabulary
answer one question: how does Lucidos talk to you. One Settings section with two
pickers adds no new concept for the user. It also inherits ADR 0216's tier
reasoning unchanged: the key is workspace-global, so the cached system block
stays shared across threads.

**Its own key, not an entry in the style library.** The library is open and
user-written; literacy is a closed set. Folding literacy into the library would
make combined styles, one per style and level. Two keys joined at render time
give every combination, custom styles included, with nothing authored.

**Coding agents get it because the request said all communication.** A
coding-agent session talks to the same person. The style stays out because a
coding agent's length is governed by its own instructions, as ADR 0216 argued.
Read at spawn rather than per message, because the session's system prompt is
fixed for its lifetime, and a level rarely changes.

**Stated, never inferred.** A stored level colours every later answer. A fluent
writer may still want plain words, and a terse one may be a developer. A wrong
guess is invisible to the user and hard to correct, since nothing tells them it
was made. So the setup interview asks with a card, and the agent sets the key
only when the user says their level.

**A closed set of three, not free text.** Each level has engine-authored wording
under the response style's floor, so no level can drop a warning or a step. The
set can be checked across the catalog, the Settings picker and the interview
knowhow, and a test does. Free text would be the `user_profile.md` workaround
ADR 0216 retired.

**Mandatory, but only the level.** The level is a fact about the person, and
every word depends on it, so the first chat asks. Length has a safe default
(Standard), so asking it too would add a question before the user has seen a
single answer.

Setup mode says "do not proceed", and automation has nobody to answer. So only
a turn a person sent from a device enters it. A trigger fire, a sub-thread and
a cross-workspace task carry no sending device. Without that
gate, every existing workspace's automation would stall until the user next
chatted, since no workspace has the key on upgrade.

**Words, not amount.** A technical user who wants only the outcome is a real
case. A level that also asked for more detail would force them to lie about
their level to get short answers. So the levels speak only about vocabulary,
and the style owns how much.

**Relevance, not autonomy.** A non-technical user was asked what to do with a
git branch, a question they could not parse. So the plain level never gets a
plumbing question: the agent decides and says what happened in their terms.
That is about which questions make sense, and it follows from the level.

**Coding agents are an expert tool.** As models improve, the Lucidos Agent
handles most assistant work, and coding agents are for complex backend work.
So the plain level is steered away from them in chat. Hiding them was
rejected: a level that silently removes a feature strands anyone following a
guide.

**Minimal is the talking half of autopilot.** A technical user who wants only
the outcome picks their real level and Minimal. A separate Autopilot style would
have near-duplicated Minimal, which research found adds nothing (several of
ChatGPT's eight personality presets tested the same).

**Three levels, with pinned card wording.** The first cut had four. The
second, "comfortable with apps, files and settings", separated nobody: almost
everyone uses apps. Its rules barely differed from the lowest level either. So
it merged into **Keep it plain**, and a stored `everyday` reads as that level.

The agent also wrote its own option text and drifted into "full depth". So the
label and line for each level are pinned, in the engine and the knowhow.

## Consequences

- The always-loaded budget rose by 766 characters in two steps, billed at the
  widest level on top of the widest shipped style.
- The setup interview's first card is the literacy question, and it counts
  toward the eight-card ceiling.
- A running coding-agent session keeps the level it started with.
- No per-thread or per-trigger override, for ADR 0216's reasons.

## Alternatives considered

**A line in `user_profile.md`.** The smallest change: the interview writes one
sentence. Rejected because coding agents never see the profile, and the profile
rides in the message tail as context, where the model can drift from it. Nothing
can check free text either.

**A separate setting under Settings → Models.** The first draft of the plan.
Rejected by the maintainer as one more concept, when the response style already
answers the same question.

**Inferring the level from how the user writes.** Rejected: see "Stated, never
inferred" above.

**An involvement setting now.** How many technical choices a technical user
wants to weigh in on is real: a developer may still want autopilot. Deferred,
not rejected. It belongs with the coding-agent settings, since the chat agent
already acts first. Research found no product that ties autonomy to expertise.
The permission ladders others ship answer a different question (may I run
this), which Lucidos already has.

**A third picker for purpose (autopilot, learning, explain).** Rejected in
favour of widening the style. The style library already holds free-form
instructions, so a purpose is a style, not a new control.
