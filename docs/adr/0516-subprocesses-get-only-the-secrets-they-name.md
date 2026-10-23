# 0516: Subprocesses get only the secrets they name; output masked by value and shape; key reads ask

- **Status**: Accepted
- **Date**: 2026-10-18
- **Amends**: ADR 0509 (its "Not protected against" list)

## Context

ADR 0509 sealed stored secrets at rest and gave subprocesses a database role
that cannot read them. Agents still reached secrets three other ways:

1. **Blanket injection.** Every `run_bash`, `run_bash_background`, `run_python`
   and scheduled script got every credential as `CRED_*` and every OAuth
   account as `OAUTH_*`. An `env` dump showed them all, and a tool result
   stored the dump.
2. **Inherited environment.** Each of those spawns inherited the engine's whole
   process environment. A token in the shell that launched a dev engine reached
   every chat tool. Output masking covered only the injected values.
3. **The key files.** Nothing stopped a command from reading
   `~/.lucidos/secret-keys/` or the shared Postgres password file.

Plan: `docs/plans/2026-10-18-secrets-reach-agents-only-by-name.md`.

## Decision

1. **A chat or script subprocess gets a `CRED_*` or `OAUTH_*` variable only
   when its own text names it.** That text is the command or code, plus each
   workspace `.py` / `.sh` file the command names and the scripts beside that
   file. A *secret mention* is the
   variable as a whole word, or a credential's custom alias.
2. **Inherited secret-shaped variables are stripped** from chat and script
   subprocesses, and every subprocess output is masked by value and by token
   shape.
3. **A command or file read naming the key directory or the shared Postgres
   password file asks.** No stored grant covers it, and a trigger or an
   unattended coding agent is refused.

## Rationale

**Naming is the declaration.** A script that reads `CRED_X` literally keeps
working with no edit, and it needs no manifest field. A name built at run time
is declared with a header comment, `# lucidos-credentials: CRED_A CRED_B`,
which is just another mention. A mention injects the whole credential, so
naming either half of a password pair sets both.

**An unknown name runs, with a note.** Failing the call would break commands
that mention a name without reading it, like `grep -r CRED_MAIL knowhow/`. The
note lists the names the agent can use, never a value. A bare `CRED_` or
`OAUTH_`, as in `env | grep CRED_`, names nothing and gets the same note. Names
are not secret: the chat prompt already lists each stored service.

**Discovery is `lucidos credentials list`.** It shows each credential's
variable names, computed by the function the injection uses. A list in the tool
description would cost tokens on every turn for a lookup most turns never need.

**Naming needs no approval.** The user decided this. The agent already runs
arbitrary commands, so a card per credential would add friction with little
gain. The proxy `script_handshake` keeps its own approvals record.

**Masking covers what stripping cannot.** Coding agents keep their inherited
environment, because Claude Code and Codex may sign in with an inherited
provider key. So their tool calls, permission cards and results are masked
instead, through `redact_subprocess_output_in_json` and `stored_tool_output`.
A masked card hides the token from the approver too, which the user chose. `core::redact_subprocess_output` is the one entry point:
the value mask, then the shape mask, then `redact_postgres_secrets`.

### Where the build departed from the plan

- **Any env-shaped word loads credentials.** `SecretMentions::wants_credentials`
  is true for any word of capitals, digits and `_`. A custom alias such as
  `GITHUB_TOKEN`, or even `TOKEN`, must inject when named, and only the
  database knows aliases.
- **A named script brings its siblings.** A trigger's `run.sh` that calls
  `python3 fetch.py`, or a script importing a sibling module, needs what the
  sibling names. A module imported from another directory is not followed: it
  declares its names with the header comment. The OAuth load stays gated on an `OAUTH_*` mention.
- **The inherited snapshot is process-wide.** `core::inherited_secret_env` is a
  `OnceLock`, because the process environment it snapshots is process-wide.
  Tests pass an `InheritedSecretEnv` explicitly.
- **The strip test runs a real child.** `core::environment_variables` tests
  spawn `env`. Source-scan tests in `bash.rs` and `scripts.rs` pin that the
  chat and script spawns pass `Strip`.
- **A name-guessed value under 8 characters is not masked**
  (`MIN_GUESSED_SECRET_LEN`). Masking a short value such as `true` would hide
  that word everywhere in the output.
- **A key read is gated even with the `command_guard` preference off.** The
  guard ships off, so the chat lane would otherwise get no protection.
- **`SecretRead` has no wire form.** `SideEffectCategory::SecretRead` carries
  `#[serde(skip)]`. A trigger grant naming it fails to parse, so nothing can
  grant it, and no TypeScript changed.
- **A key read settles as `StaticVerdict::SettledIrreversible(SecretRead)`**, a
  new variant, rather than a category added to `Settled`.
- **The path match ends on a name boundary.** `.lucidos/secret-keys-notes` is
  not a key read. Quotes and backslashes are dropped, and `/./` and `//`
  collapse, before matching.

## Consequences

**ADR 0509's "Not protected against" list changes.** Its line about secrets the
engine hands out by design now reads: a subprocess that names a `CRED_*` or
`OAUTH_*` variable, the `http_request` tool, and proxy credential injection.

**Stored events are not scrubbed.** Tool results recorded before this change
may hold tokens. Events are append-only, and the user chose to rotate the
leaked tokens at their providers instead. This is a deliberate no.

**The key-read rule is a stopgap.** A static rule cannot see through these:

- string building, `base64` and ANSI-C escapes;
- `cd ~/.lucidos` followed by a relative path;
- a Grep rooted at `~` or `~/.lucidos` that never names the key directory;
- a glob such as `secret-key*`.

Claude Code also settles some reads itself, when a broad `Bash` or `Read` sits
in `--allowedTools`. The engine never sees those.

**Follow-up: an OS sandbox.** The real boundary for path 3 is a sandbox around
subprocesses (Seatbelt on macOS, Landlock or bubblewrap on Linux), or a
separate OS user. It is not built.

**Known holes in value masking stay.** A token split across two
`bash_background` drain windows, `Stream::push` trimming a token's front, and
the raw copy under `.lucidos/exhaust/`. They are documented at the masking code.

## Alternatives considered

**A credential field in app manifests and trigger definitions.** It adds a
second place to declare what the text already says, and drifts from it.

**Failing a call that names an unknown credential.** It breaks commands that
mention a name without reading it.

**Listing credential names in the `run_bash` and `run_python` descriptions.**
Always visible, but it costs tokens on every turn and needs a per-workspace
description. The CLI list answers on demand.

**An approval card per credential use.** Declined by the user, as above.

**Scrubbing secrets from stored events.** It rewrites append-only history, and
a leaked token needs rotating either way.

**Stripping inherited secrets from coding agents too.** They may need an
inherited provider key to run at all.
