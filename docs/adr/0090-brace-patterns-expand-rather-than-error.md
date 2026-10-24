# 0090: Brace glob patterns expand rather than erroring, so the model keeps writing the syntax every other tool accepts

- **Status**: Accepted
- **Date**: 2026-08-18

## Context

`glob_files` and `grep_files` compiled their patterns with `glob::Pattern`,
which has no brace expansion and matches `{` and `}` as ordinary characters. A
pattern like `artifacts/{one,two}/**` therefore matched nothing and returned
`{"paths":[],"truncated":false}`.

That answer is indistinguishable from a correct empty one, so the model
concluded the files did not exist and moved on. Measured over 30 days of one
workspace's event store, **every brace pattern ever sent to `glob_files`
returned nothing: 14 calls, no exceptions.** Empty rate by pattern shape, over
240 calls:

| Pattern shape | Empty rate |
|---|---:|
| Trailing `**` | 6.6% |
| Plain `*` or `?` | 9.7% |
| `**/` in the middle | 21.6% |
| Any brace | 100% |

It was syntax rather than absence. In the same window the same directories
answered the plain form: `artifacts/<name>/**` hit for three of the directories
that the one brace pattern listed together and got nothing for.

The model writes braces because every other glob it has met supports them:
bash, zsh, minimatch, ripgrep's `--glob`, and the file tools in every coding
agent it has been trained on.

## Decision

Expand braces before compiling. One shared helper in `engine/tools/search.rs`
turns a pattern into one `glob::Pattern` per alternative, and a path matches
when any of them matches. Both `glob_files` and `grep_files`'s `path_glob`
compile through it, so the two cannot drift on what a pattern means.

The dialect is bash's, and the expansion is capped at 256 alternatives with an
error past the cap.

## Rationale

**Why expansion and not an error.** The point is that the model should keep
writing the syntax every other tool accepts, rather than learning a Lucidos
dialect. An error converts a silent wrong answer into a loud one, which beats
today. It still spends a round trip on a lesson no other tool would teach.

**Why bash semantics rather than our own.** A brace parser has to settle four
cases: nesting, a group with no comma, an unbalanced brace, and an empty
alternative. The model already carries an answer for each. Bash reads `a/{b}`
and an unbalanced `{` as literal text, and reads `x{,.md}` as `x` and `x.md`.
Matching that costs nothing and surprises nobody. Each rule is pinned by a test.

**Why braces inside `[...]` stay literal, which bash does not do.** Bash
expands the text before any character class exists, so it turns `[{]a,b[}]`
into `[]a]` and `b[]`. It has quoting for the literal case and we do not: our
whole input is one pattern string. `glob::Pattern` has no backslash escape
either, so a hand-rolled one would be a dialect of exactly the kind this change
removes. That leaves the character class, which already means a literal `{` to
the matcher, as the only escape we can offer.

**Why a cap.** Nesting multiplies: nine two-way groups reach 512 alternatives,
and each one costs a compile plus a comparison per file walked. The expansion
stops as soon as the count could pass 256, rather than building the list and
measuring it afterwards.

## Consequences

- The tool descriptions the model reads now state that braces alternate. The
  contract it reads and the one it gets are the same.
- A pattern past the cap errors, naming the cap and asking for a narrower one.
  This is the one brace shape still refused.
- One case that used to work no longer does. A file whose name really contains
  `{one,two}` was matched by a pattern spelling it out, because both sides were
  literal. That pattern now expands, and the file is reached with
  `[{]one,two[}]` instead.
- Expansion happens before compilation, so an alternative can carry an escape
  the raw pattern hid: `{/etc,artifacts}/**` looks relative until it is split.
  Every expanded alternative is re-validated against the `data/` boundary, not
  just the pattern the caller sent.
- The single pass over `list_searchable_data_files` is unchanged, so ordering,
  the `GLOB_LIMIT` cap and the `truncated` flag behave exactly as before. The
  walk is never repeated per alternative.
- `api/data_api.rs` compiles its own pattern for the `/api/v1/data` listing and
  is left alone. It is a different surface with a different caller, and no
  brace pattern has been observed reaching it.

## Alternatives considered

**Reject a brace pattern with an error naming the unsupported syntax.** The
smaller change, and it removes the silent failure just as completely. The model
gets something it can act on instead of an empty list. Rejected because it
leaves a Lucidos-only rule to remember, and every brace pattern still costs a
wasted round trip before the retry. The fork was put to the user, who chose
expansion.

**Swap `glob` for a crate with brace support.** `globset` and `wax` both handle
alternation. Rejected on blast radius: `glob::Pattern` also backs the backup
ignore rules, the import filter and the data listing. A matcher swap changes
what every pattern means in all of them, for one tool's bug.

**Expand and walk once per alternative.** The obvious shape, and the one that
needs no thought about the cap. Rejected because it multiplies the tree walk by
the alternative count. Ordering, the limit and the `truncated` flag all assume
one pass over a sorted list.

**Document the limitation in the tool description instead.** Cheapest of all,
and rejected on evidence. Guidance in a description competes with the syntax
the model learned everywhere else. The failure is silent, so nothing corrects
it when the guidance loses.
