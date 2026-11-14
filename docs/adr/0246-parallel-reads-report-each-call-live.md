# 0246: Batched read-only tool calls run concurrently and report each call live; async tools rejected on measured data

- **Status**: Accepted
- **Date**: 2026-09-22

## Context

Unreal Agent, a new agent harness, claims up to 40% lower cost than Codex from
three ideas. Tools run asynchronously, independent of model turns. The agent
has a single bash tool. The prompt asks the model to batch independent calls.
The plans are
[`docs/plans/2026-09-22-batched-and-parallel-read-tool-calls.md`](../plans/2026-09-22-batched-and-parallel-read-tool-calls.md)
and
[`docs/plans/2026-09-23-honest-parallel-steps.md`](../plans/2026-09-23-honest-parallel-steps.md).

The Lucidos Agent ran every tool of a batch in sequence, and its prompt asked
for batching only when writing N files. Fourteen days of one dev workspace:

| Measure | Value |
|---|---|
| Rounds with exactly one tool call | 85% |
| Median model round between tool calls | 7.6 s |
| Median `run_bash` call | 1.0 s (p90 8.4 s) |
| Input tokens per round | ~134k, 93% cache reads |
| Repeat `bash_output` polls | 125, about 1% of rounds |
| Claude Code share of all input tokens | ~90% |

## Decision

**The prompt asks for every independent call in one response. A block of
consecutive pure reads in that response runs concurrently, up to four at once.
Each call emits `ToolCalled` when it starts and `ToolResult` the moment it
finishes, so every step row shows what really happened.**

Every reader that pairs a result with its call does so by the result's
`tool_called_event_id`. The allowlist is `PARALLEL_SAFE_TOOLS` in
`agentic_loop/helpers.rs`, and a grouped tool counts when its `action` resolves
to a listed name.

## Rationale

**The model round is the slow part, so batching is the lever.** A round costs
7.6 s against about 1 s for a tool, and every round resends the whole context.
Parallel execution only overlaps tool time, so it is the smaller half.

**An honest transcript needs results in completion order.** The first version
kept the events interleaved in call order. Only the first step of a run spun,
and the rest looked finished only once the whole run ended. Reporting each call
as it goes fixes that. It also means a result can land before an earlier
call's, and position no longer pairs it.

**So every affected reader pairs by id.** Six did it by position:

- the resume history builder, which would hand the model swapped file contents
- the orphan sweep, which would stub the wrong call after a crash
- the steps rebuild and the conversation view, which would mark the wrong step
  failed
- the rerun summary, which would name the wrong call beside each result
- both transcript step resolvers, which would tick off the wrong row

Each keeps its positional rule for legacy rows with no id. A result naming a
call that is not pending pairs nothing, because guessing would hand it to
another call.

**Only pure reads, and only consecutive ones.** A listed tool has no command
guard lane, no special-tool routing, no thread event while it runs, and no
write. A run never crosses another call, so a read after a write still sees the
write. That also keeps seven more readers sequential by construction: the
permission cards, the question lookup and the file-write preview only ever see
calls that run alone.

**The emits stay outside the cancel race.** Stop drops each call's work, never
its `ToolCalled` or `ToolResult`, so no step can be left spinning.

**The executor is unordered underneath.** An ordered buffer holds a finished
call's slot until every earlier call is done. One slow call would then delay
the start of every call behind it. Outputs are sorted back into call order,
which is the order the model receives them in.

## Consequences

- Rows of a run can land in any order: two concurrent inserts commit in either
  order, and results arrive as calls finish. The frontend sorts by timestamp and
  sequence, and its incremental cache rebuilds when order breaks.
- `ResponseEvent::Step` carries `tool_called_event_id`, mirroring `Step`. It is
  additive on the wire.
- Only an async handler overlaps: `glob_files`, `grep_files`, web search, news
  and the event queries. `read_file` and `list_files` do synchronous I/O, so
  they still run one after another inside a run. They stay listed so a run of
  mixed reads is not split around them.
- `run_bash`, `run_python`, MCP, browser, email and every write still run one
  at a time.
- Adding a tool to the allowlist means checking the four properties above.
  Tests pin the guard and routing halves.
- The always-loaded prompt grew by 110 characters for the wider rule.

## Alternatives considered

**Async tools, Unreal style.** Every call returns "in progress" at once, and the
harness calls the model again as each result lands. Rejected on the data: our
tools finish in seconds, so there is nothing to overlap, and a model call per
arrival adds rounds. The polling tax it removes is 125 repeat polls in 14 days.
Long work already has an async path: `run_bash_background` plus the event wake
(ADR 0049), and child threads.

**A single bash tool.** Rejected by ADR 0088: the agent's value lives in its
domain tools, and the cached tool array costs little per round.

**Keep the interleaved order and change no reader.** The first version did
this. Rejected because the transcript lied: the user saw one spinner for a
batch of calls, and finished calls waiting on the slowest.

**Run the whole batch concurrently, not only consecutive reads.** Rejected: a
read would overtake a write that precedes it in the same response.
