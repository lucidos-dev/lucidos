Coding-agent sessions under Claude Code keep ending their turn on a dead job.
A session starts a long command or a subagent in the background. Claude
Code's tool result then says it will be notified when the job finishes. The
model believes that and ends the turn. But the engine kills the whole process
group when the turn ends, so the job dies and no notification ever comes. The
thread then looks finished with the work half done.

Our system prompt already warns about this, but the models trust the tool
result over the prompt.

Claude Code has a switch for this: `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1`
in its environment. With it, the `Bash` and `Agent` tools lose their
`run_in_background` parameter, and passing it fails validation. A command
that outruns its timeout is killed instead of moved to the background.

Every Claude Code session the engine spawns should run with background tasks
off. A user's own env vars must not be able to turn them back on. Our prompt
to the model must stay accurate once the switch is on.
