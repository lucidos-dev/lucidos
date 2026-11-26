---
name: Building a Software Factory
description: Load when the user wants a delivery process for their repository: "set up our dev workflow", "track stages", "show where each change is". Stage events, triggers, hooks, stage maps.
---

<!-- domain-event: TestsStarted: Example domain event a repository's test script posts when a test run starts. -->
<!-- domain-event: TestsPassed: Example domain event a repository's test script posts when a test run passes. -->
<!-- domain-event: TestsFailed: Example domain event a repository's test script posts when a test run fails. -->

# Building a Software Factory

A *software factory* moves a repository's changes through named stages. Its
rules hold work back until they are met. Lucidos supplies the parts. The
factory itself is workspace content: events, triggers, hooks and an app, all
yours.

Build it in this order. Each step works on its own, so stop where the user's
need ends.

## 1. Name the stages, and post their facts

A stage starts and ends at an event. Lucidos already records the facts every
repository shares:

| Fact | Event |
|---|---|
| A plan or design starts | `PlanStarted` |
| A plan is approved | `PlanApproved` |
| A change is proposed | `ChangeProposed` |
| Work is held back from a proposal | `ProposalWithheld` |
| An Apply is asked for | `ChangeApplyRequested` |
| A change waits in the landing lane | `ChangeQueued` |
| An Apply fails | `ChangeApplyFailed` |
| A change lands on main | `ChangeApplied` |

A step only your repository takes posts its own *domain events*. Name a pair
for each stage, in the past tense: `TestsStarted`, then `TestsPassed` or
`TestsFailed`. The repository's script posts them:

```bash
lucidos events emit TestsStarted --summary "Tests started" \
  --payload '{"thread_id": "'"$LUCIDOS_THREAD_ID"'", "head_sha": "'"$(git rev-parse HEAD)"'"}'
```

Put `thread_id` in each payload, so every fact ties to the thread whose change
it measures. Never post a fact Lucidos records itself.

## 2. Mark checkpoints in the flows

A coding agent follows the flows the repository writes, such as "plan, build,
test, review". A *handoff checkpoint* names a point in a flow. The agent runs
`lucidos successor offer --at <name>`, and Lucidos records
`HandoffCheckpointReached` with that name. See
`system-knowhow/coding-agent-flows`.

## 3. React to stages with triggers

A trigger fires `on` any event, a stage fact or a checkpoint included. Use one
for work that follows a stage: run the tests when a change is proposed, or
open a pull request after `ChangeApplied`. A trigger cannot stop anything,
because the event is already recorded. See `system-knowhow/triggers`.

## 4. Stop progress with a hook

An *event hook* holds work back until a rule holds. It is asked before a
thread starts, before a trigger fire, before a proposal and before an Apply.
For example, a hook on `ChangeApplyRequested` can refuse an Apply until
`TestsPassed` names the change's last commit. Its script reads that with
`lucidos events query --type TestsPassed`. See `system-knowhow/event-hooks`.

Write the hook's refusal reason for the agent or person who meets it: say
what to run or fix.

## 5. Read the stages back through a stage map

A *stage map* turns a change's events into the stages it visited, how long
each took, and where it waits now. It has two parts:

- **The stages, in order.** Lucidos's own facts give a base: designing,
  planning, ready to build, implementing, ready to land, landing, landed and
  live.
- **The rules.** Each rule names an event, an optional test on its payload,
  and the stage the change enters. `PlanApproved` enters implementing,
  `ChangeProposed` ready to land, `ChangeApplyRequested` landing and
  `ChangeApplied` landed.

A factory adds a stage by placing it after an existing one and naming the
events that start and end it. `TestsStarted` could enter a testing stage after
implementing. `TestsFailed` returns the change to implementing, and
`TestsPassed` moves it on to ready to land.

The map is data your app or trigger reads, so keep it in one file in the
workspace. Lucidos's own factory keeps its map in its repository, in
`scripts/scoreboard/lucidos-stage-map.mjs`, as an extension of the base map.

## 6. Show it in an app

An app reads the events through the SDK and draws the stages through the
map. Keep the record in events, never in a field someone must update: a
hand-set status drifts from the truth within hours. See
`system-knowhow/building-an-app`.

## Questions to settle with the user before creating

- Which repository, and which stages does a change pass through today?
- Which of those does a script already run, and where would it post its facts?
- What must be true before work moves on? Each answer is a hook candidate.
- What should happen by itself after a stage? Each answer is a trigger.
- Who reads the stages: the user in an app, or a report?
