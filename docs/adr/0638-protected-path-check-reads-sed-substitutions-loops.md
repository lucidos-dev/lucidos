# 0638: The protected-path write check reads a printing sed, a read-only substitution, and loop keywords

- **Status**: Accepted; amended by [ADR 0641](0641-coding-agents-trust-by-provenance.md): the walk decides only whether a
  narrow grant covers a command
- **Date**: 2026-11-09

## Context

ADR 0634 made a persisted Bash grant cover any command that only reads under
`.claude/` or `.git/`. `command_guard::command_may_write_under` decides that,
and it erred towards a write in three places that ordinary read-only commands
hit:

- `sed` was not a read-only head, so `sed -n '64,67p' file`, or a `sed`
  trimming a pipe, counted as a write.
- Any `$(...)`, backtick or `<(...)` counted, whatever its body ran.
- Shell keywords counted as heads, so every `for`, `while` or `if` counted.

A user with a bare `Bash` grant got three such cards from one thread, each
without "Always allow". Every one named `.claude/` and only read.

## Decision

ADR 0634's model stands: a command naming a protected directory is covered
only when every segment only reads. This ADR widens what "only reads" means.

- **`sed`** reads when three things hold:
  - no in-place flag (`-i`, `-I`, `--in-place`, a short cluster holding `i`);
  - no script file (`-f`);
  - no script command that writes or runs (`w`, `W`, `e`, the `w` and `e`
    flags of `s`).

  A small parser reads the script. An unknown command or option counts as a
  write. So do the forms where GNU and BSD sed split a script differently: a
  short `-l`, and a delimiter inside a bracket expression.
- **A substitution** body is checked like a script, recursively, to a depth
  cap. Its output then stands in as a placeholder word. That word reads as
  the value of an assignment before the head, or in a `for` list. It also
  reads as an operand of a head with no writing option: `cd`, `echo`, `cat`,
  `readlink`, `realpath`, `basename`, `dirname`, `ls`, `wc`, `head`, `tail`,
  `grep`.
  Anywhere else it counts, since it may become a flag or a redirect target
  naming any path. A line with a heredoc and any substitution still counts,
  and so does a nested backtick.
- **Keywords**: a leading `do`, `then`, `else`, `elif`, `if`, `while`, `until`
  or `!` is stripped and the rest checked. `for NAME in WORDS` reads, and so do
  `done`, `fi` and `esac` with only redirects after them. `case` counts.
- **`sort`** reads with only known reading options. `-o`, `-T` and
  `--compress-program` write or run, and any unknown option counts.
- **A comment** is blanked to its line end, so it neither runs nor
  redirects. A `#` opens one only at the start of a word the shell splits. A
  quoted or escaped `#` or `;` is text. Where the walk cannot track quotes (a
  heredoc, an open quote), nothing is blanked and a `#` word counts.
- **A quoted or escaped `(`, `)`, `{` or `}`** is text, so it opens no group
  or function body.
- **Redirects**: a noclobber override (`>|`, zsh's `>!`) writes like `>`, and
  `>&word` counts, since it writes `word`. A `$'…'` word counts anywhere: it
  can spell any option or script, and the walk does not decode it.
- **git**: `merge-base`, `check-ignore` and `for-each-ref` read in this walk.
  So do a bare `git add` and `git commit`: they write only git's own index,
  objects and refs, which they do unasked when no protected path is named. A
  global option or a `GIT_` variable before the subcommand counts, since it
  can point git at another index, directory or hook. `checkout`, `restore`,
  `rm`, `mv`, `reset` and `stash` still count: they write work-tree files.
- **A code-injecting assignment** (`PATH=`, `LD_PRELOAD=` and kin) now counts
  for every head and on its own. `PATH=./bin; cat .claude/x` can run another
  binary as `cat`. So does `TMPDIR=`, where `sort` writes temporary files. A
  `for` loop over such a name counts, and so does any `printf -v`.

## Rationale

Each widening reads a construct the shell itself treats as transparent: a
keyword runs the command after it, and a substitution runs its body. Checking
the inner command keeps the rule exact, rather than trading safety for fewer
cards. A real write inside a loop, an `if` or a substitution still counts.

`sed` is the one new head. Agents use it constantly to print line ranges, and
its writing forms are few and visible in the text.

## Consequences

- Bare `Bash` users stop seeing cards for printing `sed` calls, read-only
  substitutions, and loops whose bodies read.
- `while read f` still counts: `read` is not a read-only head, and
  `read PATH` would set a code-injecting variable.
- The variable miss ADR 0634 accepts now has one more form. A substitution's
  output stored in a variable and expanded later is unchecked, as a literal
  value is: `x=$(printf -- --in-place); sed -n p $x .claude/y`.
- A corpus replay of 13,799 past commands naming a protected directory cut
  the card rate from 62% to 38%. No command flipped from covered to carded,
  and no newly covered command wrote under a protected directory.
- `git add` and `git commit` naming a `.claude/` file no longer card. Their
  hooks were already reachable by any commit, so no new path to code opens.
- `awk`, `perl` and other interpreters stay writers.

## Alternatives considered

- **Add `sed`, `sort` or the new git subcommands to the shared read-only
  lists.** Would also change the chat lane's fast path, which judges them
  elsewhere. The checks live in the protected-path walk only.
- **Track variables through the command.** Would close the variable miss, but
  needs a shell interpreter's worth of state for a case agents rarely write.
  ADR 0634 already accepts it.
- **Keep counting every substitution.** Simplest, but it carded the common
  `x=$(sed -n ...)` inside a loop, which is the shape the user hit.
