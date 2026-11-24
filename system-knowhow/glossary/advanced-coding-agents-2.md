---
name: Lucidos Glossary, Advanced (coding agents) (continued)
description: Part of Lucidos Glossary. Its index lists the sections it covers.
---

One part of `system-knowhow/glossary`, which lists every part.

## Advanced (coding agents) (continued)

### Successor session
A fresh coding-agent session that carries on a heavy thread's work in the same thread, with a clean context. It starts from a *handoff narrative*, or a *handoff brief* when the prompt cache is cold or a resume failed. A *New session* card in state "Handed off" marks where it began. It names the checkpoint, with the opening message folded under it. A request typed at the *Successor card* starts the session as the user's own message, above that card.

It may run on another implementer, Claude Code or Codex. The branch, the change and the parent's wait stay with the thread. A thread may hand off any number of times.

Five moments start one:

- picking an `Implement in a fresh session` option on a plan card;
- asking the agent to "handoff this session";
- **Continue with context** on the *Successor card*;
- an automatic *handoff checkpoint* where the *handoff estimate* says the handoff pays;
- the first safe boundary past the thread's *context ceiling*.

The agent writes the narrative, runs `lucidos successor start`, and ends its turn. The successor session then starts in the thread. No permission is needed, and uncommitted work stays in the worktree. Stopping the turn first withdraws the handoff, and so does a successor session that fails to start.

### Provider block
A turn the provider's safety classifier refused, known by the structured code on `ResponseFailed.provider_error`: `misalignment_policy_violation`, `cyber_policy` or `bio_policy`. The card names the policy and offers *Switch to Claude Code* or *Continue in a new session*. The blocked session is never resumed, so the next message starts a *successor session* from a *handoff brief*. A parent thread is told to rephrase the order or ask the user. Any other provider error code is shown but resumes as before.

See also: *Successor session*, *Handoff brief*.

### Successor thread
Retired with ADR 0560. Before *successor sessions*, a handoff started a new *coding-agent thread* instead. No new one starts, but old ones stay as they were. Its branch started at the old thread's last commit, so its *change* carries all of that work, and the old thread proposes nothing. It sits in the old thread's place: top-level if that one was, otherwise a child of the same parent, which then waits on it.

The old thread archived itself once the successor started. Its transcript ends on a "Continued in" row linking the successor, and the successor's opens on a "Continued from" row.

Writing in an old thread that is still open raises a warning on top of the composer: commits made there are set aside once the successor's change lands. **Move my message there** opens the newest thread of the chain, with the message as its unsent draft unless it already holds one. **Send here anyway** hides the warning until the composer is cleared.

A successor's title is the base title plus its position in the chain, as in "Fix the drawer (2)". A checkpoint it started at follows.

### Build chain
Retired with ADR 0560: the *legacy successor threads* that built one approved plan, children of the planning thread. A plan card's pick now starts a *successor session* in the planning thread itself. Old chains stay as they were, each builder a child of its planner.

### Handoff consent
Retired with ADR 0560: the authority a child thread's *legacy successor thread* needed to start under its parent. A *successor session* stays in its own thread, so a handoff needs no authority. Old handoffs keep their recorded consent.

### Handoff estimate
The engine's answer to "does a handoff pay here?" at a *handoff checkpoint*. Calls left are the units of work left times the calls one unit takes. A checkpoint's `--progress <done>/<total>` gives the units left, else one.

The calls per unit come from past runs of the same checkpoint in the same repository. Those runs share the model and effort, and entered at a similar context size. Without history, a default applies. Below 100k tokens it never says yes.

The estimate reads the *prompt cache*'s warmth. With a warm cache, the handoff cost adds the narrative turn's reread of the old context at the cache-read price. With a cold cache, it is only the successor's own context, and staying adds one full rewrite of the old context. The handoff pays when the calls left, saving the gap between this context and a fresh one, outweigh that cost.

### Handoff checkpoint
A named point in a coding-agent thread's work where the agent may continue in a *successor session*. A repository names its own in its plan skill, review flow or agent instructions, and Lucidos's own repo does the same. At one, the agent runs `lucidos successor offer --at <name>` and follows what it prints.

- **Automatic**: when the *handoff estimate* says the handoff pays, the agent hands off on the same implementer. Otherwise it carries on. A child thread hands off the same way, with no permission.
- **Offered** (`--ask`): the user decides on a question card, which recommends a fresh session when the estimate pays. Plan approval is the usual one.

### Implementer
The coding agent that builds a thread's work: a backend, a model and a reasoning effort, such as Claude Code on Opus 5.5 at medium effort. With no pick, a thread runs on its backend's *default implementer*. A model-only pick runs at that model's *default effort*.

A plan card recommends one for the *successor session*, as an *intelligence rung* on the *intelligence ladder*. It always does when the *handoff estimate* pays, beside the thread's own. Otherwise it offers a lower rung, or a higher one for a hard plan. `lucidos successor start` takes the choice as `--coding-agent`, `--coding-agent-model` and `--reasoning-effort`.

### Default implementer
The model and effort a coding-agent thread runs on when nobody picked one. Claude Code's comes from the `model_claude_code` and `reasoning_claude_code` *preferences*, Opus 5.5 at medium unless the owner changed it. Codex's comes from `model_codex` and `reasoning_codex`, GPT-6.2 Sol at medium unless the owner changed it. Lucidos pins it at spawn, so neither Claude Code's settings file nor Codex's config file decides a Lucidos thread's model.

### Intelligence ladder
One *ladder surface*'s model options, ordered from least to most intelligent, each an *intelligence rung*. There is one each for the Lucidos Agent, Claude Code and Codex. The shipped ladder uses Lucidos's estimates, weighted toward Artificial Analysis, to compare intelligence and cost.
A shipped rung is on when no eligible pair dominates it. The default stays on whenever available, even on a customised ladder. Shipped updates leave the saved list alone.

For the Lucidos Agent, eligibility follows the workspace's configured routes, supported efforts and known account availability.
An unavailable pair cannot suppress an available rung. The catalogue still includes unconfigured models, and saved owner lists retain their values and order.

The owner's lists are the `intelligence_ladder_chat`, `intelligence_ladder_claude_code` and `intelligence_ladder_codex` *preferences*, and **Settings → Intelligence** edits all three. The page also shows when the prices were last checked, and offers **Keep prices current**: a trigger, daily or weekly, created only when you press it. **Copy from Lucidos Agent** copies the Lucidos Agent's rungs a coding agent runs onto its ladder, once. An agent may pick only a rung that is on, and Lucidos refuses any other pick it makes. Your own *rung control* offers the same rungs that are on, so it lists no old model. `lucidos intelligence-ladder --surface <surface>` prints it.

A ladder with an owner's saved order wears a **Customised** mark, and **Reset** brings back the shipped one. When the rungs that are on do not climb in intelligence, the page says so above the rows and offers **Sort by intelligence**: one tap restates the order, keeping what is on. Nothing re-sorts without that tap.

### Rung control
The prompt-bar control for choosing a thread's model and effort. It is a small pill beside the send button, showing the model, effort ("Opus 5.5 · medium") and a chevron. The ‹ and › at its ends step one rung down or up the list, and the pill keeps one width as it steps. The compose view and a started thread draw the same pill, a phone with smaller steps. Before any icon folds into the ⋯ menu, a narrow bar sheds the effort for the rung's *band* ("1.12×"), then the name. Last go the steps and the chevron, leaving the band alone in the pill's frame, which still opens the list.

It opens a list of *intelligence rungs* from the thread's own *intelligence ladder*: each estimated rung that is on, and the pair in force when it sits off the ladder. Then come the rungs with no estimate, under *Not estimated*, and a link to **Settings → Intelligence** last. Pick one row to set model and effort together. On the Lucidos Agent, a rung whose model has a choice of backend then asks for the *preferred provider*. Codex has no slash menu, so its `/` button stays dimmed.

On a started Lucidos Agent thread, a pick is saved for the next turn, including during a running turn. The current turn keeps its model and effort. Both collapsed faces add `*` while a pick is pending; the popover names the running pair and the next-turn pair. A second pick replaces the pending one. Reload and other devices retain it, and a queued next turn reads it when it starts.

There is no Default row: while nothing is picked, the ladder's default rung wears the checkmark in its own place in the list. The pill never reads "Default" for a model either. With no model of its own, a thread names its *default implementer*. A thread that began before the backend shipped a default effort may record a model and no effort. The pill then reads "· default" when the model offers tiers.

On a started coding-agent thread, the model and effort stay fixed while a turn runs, so the control is disabled until the turn ends. A pick between turns is saved at once, and the next turn runs on it, from any device. A mid-thread change that rewrites a warm prompt cache asks first, and the confirm states the estimated extra cost of a one-time re-read. A coding-agent thread may instead hand off to a *successor session* on the picked rung. Both kinds can Keep the current pair or Switch anyway. A pick made in the compose view belongs to that draft and never changes an account *preference*.

### Intelligence rung
One option on the *intelligence ladder*: one model at one reasoning effort, such as Opus 5.5 at medium. It shows an intelligence *band*, a cost *band* and a time *band*. A rung is on or off, and only a rung that is on may be picked by an agent. A higher rung is more intelligent. The UI may say just "rung". Not a cache rung, which marks the prompt cache inside a memory view.

### Default floor
The *intelligence ladder*'s default rung, declared the lowest rung a checkpoint run may continue on. A flow declares it with `lucidos successor offer --default-floor`, as the hardening flow does. A coding-agent thread below it then hands off to a *successor session* on the default rung, which the run names as the **floor rung**. It hands off even when the *handoff estimate* does not pay. Nothing happens when the ladder does not climb, the thread runs off it, cannot hand off, or holds an open change.

### Risk rung
The *intelligence rung* just above the default, never the top, which a run declaring the *default floor* also names to a thread below it. The agent hands off on it instead for security work or a costly one-way door, and says why in one line. Security work touches a security or authority boundary, secrets, credentials, pairing, sandboxing, or what an agent may do. A costly one-way door is a migration or new stored format, a published contract, a release or install format, or destroyed work.

### Band
A rung's rough intelligence, cost or time per task, as a multiple of the default rung: `0.02×`, `0.6×`, `2.5×`. Cost and time bands move in steps of about a quarter. The intelligence band is the rung's score on its surface's *intelligence profile*, in steps of 2 points, and reads in hundredths, such as `0.96×`. Each backend compares to its own default rung. Codex falls back to Claude Code's only when its stored default is not on offer.

Cost is the rung's tokens per task at today's *price table* price. On a free route, such as OpenCode's free tier, cost reads `included`. A paid route reads a price, whoever pays, so a Claude gateway subscription reads one too. On Claude Code, a cost band includes the *long-prompt share*.

### Intelligence profile
The benchmark mix an intelligence *band* is scored on. Each rung has two. `general` weighs Artificial Analysis's Intelligence Index 75% and Epoch's ECI 25%. `coding` is Artificial Analysis's Coding Agent Index, and, where that index has no score, takes `general` plus the model's measured coding gap, marked estimated.

Coding observations come from one documented, comparable benchmark cohort. Missing efforts remain estimates; one effort's observation never becomes another's measurement.

Both profiles smooth downward fluctuations within a model: higher effort never gets a lower capability estimate. Ties remain possible. These are operational estimates, including smoothed benchmark observations.

Coding agents rank their rungs on `coding`. The Lucidos Agent's ladder ranks on `general`.

### Ladder surface
A place that picks from its own *intelligence ladder*: the Lucidos Agent, Claude Code and Codex. The UI names each by its agent, so the Lucidos Agent's ladder is never "the chat ladder". Its wire and storage key stays `chat` (`--surface chat`, `intelligence_ladder_chat`). The surfaces share one set of rung estimates, and each keeps its own on-list: the shipped one, or the owner's edit.

### Long-prompt share
The share of coding-agent calls whose prompt passes 100k tokens. Some models, such as Haiku 5.5, bill the whole call higher past that line, so their cost *band* rises with it. Lucidos measures it from the last 30 days of Claude Code calls. The slider in Settings → Intelligence overrides it, saved as the `long_prompt_share` *preference*.

### Auto-compact trial
An experiment on Claude Code threads: does a 400k-token auto-compact window cost less without hurting the work? The `coding_agent_auto_compact_trial` preference switches it. `ab` gives the window to half of all top-level threads, each together with its sub-threads and legacy successor threads. A thread keeps the arm it first ran in. `on` gives it to all of them. When Claude Code compacts its context, the step says so, and the context counter drops.

### Successor card
The card on top of the composer that offers a *successor session* to a large *coding-agent thread*. It names the thread's size and how much more each reply costs than in a fresh session. Then it asks "Continue in a fresh session or stay here?". It shows only at a clean idle: no running turn, no open question, no queued message, no event wait and no pending or withheld work. It also waits until you start writing in the composer, and hides again if you clear it. The engine measures the size and decides when the *handoff estimate* says a handoff pays.

It offers two options:

- **Continue with context** starts a successor session in this thread. Anything you typed in the composer goes along as its first request. The composer's model and reasoning picks aim the new session, not the old one. The engine picks the path by the *prompt cache*'s warmth at the tap. A warm cache has the old session write a *handoff narrative*, with a spinner until the new session starts. A cold cache starts it at once with a *handoff brief*, and the old session makes no model call.
- **Stay here** hides the card until the thread holds 100k tokens more, and a compaction below that size lifts the hold. It is a recorded decision, so a reload or another device agrees.

A picker pick under the card switches nothing. If you send in the current session instead, that send asks the usual rung switch question first.

The card flips from warm to cold by itself when the cache goes cold. Uncommitted work stays in the worktree, and the new session reads `git status` first. Right after a handoff the card hides, since it reads only the current session's size.

### Context ceiling
The context size at which a coding-agent session hands off to a *successor session*, whatever its flow says. The value is `off`, a size such as `300k`, or a share of the window such as `30%`. It never sits above 75% of the session's recorded window, nor below 100k. The default is `300k`.

- **Where it is set:** the `coding_agent_context_ceiling` *preference*, a trigger's `context_ceiling`, or the thread itself with `lucidos threads context-ceiling`. A child takes its parent's at spawn.
- **Who may raise it:** only a person raises it or turns it off. An agent may lower its own thread's.
- **When it acts:** at the first safe boundary past it, during a running turn
  when the driver can steer, or at turn end. It never interrupts a running tool.
  Unread input and open question or permission cards defer steering.
  A *hold* defers it until Resume. The session writes its *handoff narrative*
  while the cache is warm. If it writes none, Lucidos uses a *handoff brief*.
- **The opening guard:** a session already above its ceiling on its first capture
  is exempt. A session that grows past it during its first turn still hands off.
- **Stop stays stopped:** Lucidos records a brief without starting another agent
  turn. Your next input starts the successor.
- **When the successor starts:** after the narrative turn if ceiling re-entry interrupted running work. After a natural turn end, or with an engine brief, it waits for the next input, such as your message or a wake. Until then the old session takes no input.

Above the composer after a ceiling handoff, **Start fresh** moves your draft to a new compose view on the same repository and backend. It leaves your draft unsent and the old thread idle.

Above the composer, **Return to the old session** withdraws the handoff and raises the ceiling to the session's size plus 100k.

### Handoff brief
What the engine writes as a *successor session*'s first message in place of a *handoff narrative*: when the *prompt cache* is cold, or when a resume failed. It says why the context did not carry over, and that the worktree holds all the work. It tells the session to rebuild only what it needs: `git status`, the thread's recent turns, the branch's git log and any plan under `docs/plans/`.

With no request typed, the session ends its first turn on a question card. The card says where things stand and offers the open items as options.

### Handoff narrative
What an agent writes for its *successor session*: what was learned, what was settled and why, what bit it, where things stand, what to do first, and the files that matter. It has six sections, Outcomes, Decisions, Gotchas, State, Next step and Files, and Lucidos refuses one that lacks any. It becomes the session's first message.

