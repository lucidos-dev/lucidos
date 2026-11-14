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
`<phases>` counts the plan's phases. Use your normal user question tool with
relevant choices from the shape it prints. Follow-up questions may choose the
backend and then the rung.

Always include the option to approve here and `Commit plan for later`.
Include the other installed backend when its ladder was read, unless the
recommended option already names it. A typed reply requests changes; it needs
no option. A real fork that has no slot is named in the question as a typed
reply. A re-plan runs this checkpoint again before it asks again.

**A fresh-session pick.** It approves implementation: record the approval as
this repository does. Then run `lucidos successor start --narrative <file>`
with the flags the checkpoint printed, and end your turn. The narrative names
the plan's path, says the user approved implementation, and quotes the phase
checkpoint below.

**Commit plan for later.** Record plan-only approval as this repository does
(`lucidos planned approve --plan-only` in Lucidos), commit only the plan, and
end the turn. Never edit source or begin implementation. A later implementer
proposes the applied plan and asks for implementation approval anew.

**Phase commit.** After you commit a phase that another phase follows, run
`lucidos successor offer --at phase-commit --progress <done>/<phases>`, where
`<done>` counts the phases committed. Follow what it prints. Skip it after the
last phase. Other-backend automatic rungs are taken only when the user, plan
or approval pick named that backend; otherwise follow the own-backend verdict.
<!-- /default-flow: implementation-plan -->
