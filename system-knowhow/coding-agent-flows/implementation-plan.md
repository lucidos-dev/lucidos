---
name: Default flow: implementation plan
description: The default handoff checkpoints for a plan flow. An offered checkpoint at plan approval, and an automatic one after each phase another follows.
---

# Default flow: implementation plan

A reference of `system-knowhow/coding-agent-flows`, which says how to take it.
Copy the block below into the plan flow, markers included. The repository's
own steps, such as how it records an approval, stay outside the block.

<!-- default-flow: implementation-plan -->
#### Handoff checkpoints

**Plan approval.** Before you ask for approval, run
`lucidos successor offer --at plan-approval --ask --progress 0/<phases>`, where
`<phases>` counts the plan's phases. Ask with your question tool, in the card
shape it prints. The option to stay is `Approve here`. With no fresh-session
option, the floor is `Approve` / `Request changes`, and a real fork takes the
second slot. A re-plan runs this checkpoint again before it asks again.

**A fresh-session pick.** It approves the plan: record the approval as this
repository does. Then run `lucidos successor start --narrative <file>` with
the flags the checkpoint printed, and end your turn. The narrative names the
plan's path, says the user approved it, and quotes the phase checkpoint below.

**Phase commit.** After you commit a phase that another phase follows, run
`lucidos successor offer --at phase-commit --progress <done>/<phases>`, where
`<done>` counts the phases committed. Follow what it prints. Skip it after the
last phase.
<!-- /default-flow: implementation-plan -->
