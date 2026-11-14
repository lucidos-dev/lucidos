# 0670: A literal heredoc body that nothing in the command runs is not a secret read

- **Status**: Accepted
- **Date**: 2026-11-12

## Context

An agent wrote a handoff narrative with `cat > /tmp/h.md <<'EOF'` and then ran
`lucidos successor start --narrative /tmp/h.md`. The narrative warned "Do NOT
read `~/.lucidos/gateway/gateway.log`". The command carded as a secret read with
no grant offer, twice, and the owner allowed it both times.

`command_reads_secret` ran `mentions_secret_path` over the whole command text,
heredoc bodies included. A body behind a quoted delimiter is literal text. When
the command only stores it, nothing reads the path it names.

The guard already places such bodies: `blank_literal_heredoc_bodies` blanks
them for the fast path, and refuses every shape it could misplace. Its doc keeps
it away from the danger scans, because the head may run the body as code
(`sh <<'E'`).

## Decision

The secret scan skips a literal heredoc body only when nothing in the same
command can run it. Three things must hold, else the whole text is scanned as
before:

- every body is placed;
- no substitution remains;
- every segment head, after plain assignments, is on a closed data-sink list,
  and no segment holds a parenthesis.

A parenthesis voids the rule because `cat () ( bash )` turns a later `cat`
into a shell. The body walk ends a delimiter word only at a space, tab or
newline, as the shell does, so a CRLF opener keeps its `\r`.

The list is `cat`, `tee`, `printf`, `echo`, `mkdir`, `ls`, `wc`, `head`, `tail`,
`true`, and `lucidos successor start|offer` or `lucidos data write`.

A shell wrapper gets the rule for its script only when the outer shell expands
nothing in the `-c` operand: single-quoted runs and backslash escapes only,
which is how Codex renders argv. The preamble and the positional tail are
scanned raw.

Python code keeps the whole-text scan. In Python, `cat <<'E'` is a shift, and
the lines a shell would read as a body run.

## Rationale

The rule is structural. Quoting alone proves nothing, since `bash <<'E'` runs
its literal body. A closed list of heads that only copy their input settles
the question without reading intent from prose. Each refused shape falls back
to the old scan, so the change can only remove false positives on commands that
write data. It cannot hide a read the old scan caught.

## Consequences

- A command that writes a narrative naming a secret path now takes the normal
  path: Safe, a judge verdict, or a card with a grant offer.
- A real read stays `SecretRead` and never grant-covered. That covers:
  - a direct read, an input redirect, a here-string, a write into a secret path;
  - an unquoted body, which expands;
  - a body run by its head or by a later segment;
  - a substitution beside the body;
  - a double-quoted wrapper, and a wrapper tail.
- The cross-command gap is unchanged. A narrative written in one command and
  run as a script in the next is caught only by the OS sandbox (ADR 0645).

## Alternatives considered

1. **Exempt `lucidos successor start` narratives by path.** Rejected. It names
   one verb, and every other data write still cards.
2. **Skip any quoted-delimiter body.** Rejected. `bash <<'E'`, `sh -s` and
   `cat <<'E' | sh` all run a literal body.
3. **Read the body's prose for negation.** Rejected. Sentiment is no proof, and
   a model can phrase a read any way it likes.
4. **Apply the rule to Python code too.** Rejected, as above: Python has no
   heredoc, and a `cat` object with `__lshift__` runs the "body".
