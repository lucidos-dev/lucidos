# 0535: Claude Code sessions run without the tools that promise later work

- **Status**: Accepted
- **Date**: 2026-10-20

## Context

The engine ends a coding-agent session's whole process group when its turn
ends ([0358](0358-claude-code-runs-with-background-tasks-off.md)). Nothing
the session scheduled can fire afterwards.

Claude Code still offers five tools that promise exactly that:

- `ScheduleWakeup` resumes the session after a delay.
- `CronCreate`, `CronDelete` and `CronList` manage recurring prompts.
- `Workflow` runs a script in the background and reports back later.

The workspace event log held 24 `ScheduleWakeup` calls from coding-agent
threads. Every one used it as a wait, as a "fallback in case the event wait
does not fire". None could fire. No `Workflow` or `Cron*` call appears.

The tools also cost tokens on every request. Claude Code's own `/context`
shows the loaded schemas as a 2.6k-token row, and the deferred names as 6.1k
more. The skills that drive them (`loop`, `workflow-authoring`) sit in the
skill listing too.

## Decision

Every Claude Code spawn passes `--disallowedTools` with the five tool names,
built in one place (`DISALLOWED_TOOLS` in `runtime/claude_code.rs`). Resumes
and side questions pass it too.

## Rationale

It is the same reasoning as 0358: remove the false promise at its source. A
rule in the prompt loses to a tool that is right there in the tool list.

- The flag removes the tools from the list the model sees. The session cannot
  call a wake-up that never fires.
- A side question resumes the session's own transcript to read it from cache.
  It must send the same tool list, so it carries the flag as well.
- The baseline shrinks on every request of every session.

The waits that work stay: a foreground call with the maximum timeout, and
`lucidos background-task run`, which the engine owns and which re-opens the
thread.

## Consequences

- `ListAgents`, `SendMessage`, `EnterWorktree` and `EnterPlanMode` stay. Each
  can still work inside one turn.
- A skill that needs a removed tool is switched off in the engine's base
  settings (`skillOverrides` in `cc_settings.rs`), so the listing does not
  advertise it.
- A future Claude Code tool that promises later work joins the list in
  `DISALLOWED_TOOLS`. A test pins that the list never names a tool a Lucidos
  session needs.

## Alternatives considered

- **Prompt wording that forbids the tools.** The prompt already tells sessions
  to wait with a foreground call or a background task. A tool in the list
  still wins, and the schemas stay in the baseline.
- **A `PreToolUse` hook that refuses them.** The model would still pay for the
  schemas and still try the call. A refusal costs a turn.
- **Leaving the tools and trimming the skills only.** It keeps the false
  promise, which is the actual defect.
