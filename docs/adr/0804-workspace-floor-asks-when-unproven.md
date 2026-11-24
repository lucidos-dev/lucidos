# 0804: The workspace floor refuses proven destruction and asks once when only conservative evidence matches

- **Status**: Accepted
- **Date**: 2026-11-23

## Context

The workspace floor protects workspace directories, their databases and the gateway routes that can delete them.
Its conservative scan previously hard-refused commands based on text matches alone.
This blocked source edits and commit messages that quoted destructive commands.

## Decision

This amends decision 7 of [ADR 0728](0728-command-guard-one-owner-per-harm.md).

| Workspace floor verdict | Evidence | Action |
|---|---|---|
| `Proven` | A parsed destructive operation targets a workspace, its database or a gateway delete route. | Refuse on every lane. |
| `Unproven` | Only conservative evidence matches. | Ask the owner to allow this one command. Refuse on lanes that cannot ask. |
| `Clear` | The workspace floor found neither kind of evidence. | Proceed to the remaining command checks. |

The coding-agent card offers **Allow once** and **Deny** and shows the floor reason.
The chat gate settles `UnprovenWorkspaceDestruction` before the judge.
Standing and trigger grants, judge verdicts and worktree auto-allow cannot cover `Unproven`.

The filesystem scan reads live operations, including command substitutions and shell-outs in Python.
Printed text, commit messages supplied on stdin and Python string literals used as data yield no filesystem operations.
A destructive target in heredoc text a sink writes to a file is `Unproven`, since another command may execute the file.

For database operations, the floor can prove a literal shell variable used by `dropdb`, `pg_ctl` or `psql`.
It also reads literal `echo` or `printf` input piped to `psql`, and literal `os.system` shell-outs with whitespace before the parenthesis.
These proofs can only promote `Unproven` to `Proven`.

## Rationale

Parsed operations justify a refusal. A conservative match alone warrants explicit consent for that invocation.
Following the consumer of stdin distinguishes code from text that happens to contain command syntax.

## Consequences

The same rules apply to Claude Code, Codex and Lucidos Agent command tools on macOS and Linux.
Triggers and the coding-agent background task lane refuse unproven commands because they cannot ask synchronously.
An MCP launch checked by the command guard also refuses an unproven match because its approval persists across starts.

Shell compounds, generated scripts and shadowed commands can still produce conservative matches.
The floor asks when nesting exceeds its scan limit.
Captured sink output can also trigger a request when it contains a workspace target.
The standing corpus pins all three outcomes without running the fixture commands.

## Alternatives considered

- Hard-refuse conservative matches. This blocked legitimate edits with no way for the owner to let them proceed.
- Clear conservative matches. This would let commands run without consent when the scan cannot establish what they execute.
