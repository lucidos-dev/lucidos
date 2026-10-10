# 0095: A permission grant is scoped to the context it was made in, so the three grant files move from the machine-global user dir to the workspace; this is semantic isolation, explicitly not a security boundary

- **Status**: Accepted
- **Date**: 2026-08-19

## Context

Three files under `~/.lucidos/` recorded every "Always allow" the user had ever
clicked: `agent-allowed-commands` (the Lucidos Agent command guard),
`cc-allowed-tools` (Claude Code's `--allowedTools`) and `mcp-allowed-tools` (the
Lucidos Agent's MCP gate). All three resolved against `user_dir()`, which is one
directory per machine.

The failure was not that one workspace could reach another's data. It was that
**a yes said while looking at one workspace silently bound in every other
workspace**, including ones that did not exist yet. The live files made it
concrete. `agent-allowed-commands` held bare `Bash` and `Python`, so every shell
and every Python call auto-allowed everywhere, with no card. `cc-allowed-tools`
held an employer-specific host grant and two workplace MCP integrations, in
force inside personal workspaces.

Nobody makes those decisions for a workspace they have not created yet.

## Decision

A permission grant is stored in the workspace it was made in:
`<workspace>/.lucidos/`. A one-time migration copies the machine-global files
into every workspace that exists when it runs, and only those.

Two things this deliberately is not.

It is **not a security boundary**. Every workspace runs as the same uid and has
`run_bash`, so any workspace can `cat` any other workspace's grants whatever the
layout. There is no containment to be had between two things sharing a uid and a
shell. The property bought is that **no decision binds outside the context it
was made in**, and nothing stronger. A later reader must not build on a boundary
that is not there. If containment is ever actually needed, the answer is separate
uids or separate machines, not file layout.

The sharpest form of that, since a reviewer will raise it: an agent can append
to its own grant file and widen what it may do next. True, and true before this
change too. The command guard classifies an out-of-workspace **append** as safe
and wanted, so `>> ~/.lucidos/agent-allowed-commands` was never gated either.
Only out-of-workspace *destruction* is, and truncating a grant file removes
grants rather than adding them.

It is **not the end of `~/.lucidos/`**. The sorting rule is decision versus
resource. A resource is something the machine has exactly one of, where sharing
is the point: the gateway, the port space, the build slots, the network config.
Those stay. A decision is something the user chose in a particular context, and
its scope must be the scope of what they were looking at.

## Rationale

**Why the workspace and not `data/config/`.** `data/` is git-tracked and backed
up, which is attractive, and it is also **writable by the agent's own
`write_file` and `edit_file`**. A permission file the agent can rewrite is not a
permission file. `<workspace>/.lucidos/` is reachable by the engine and refused
by the file tools, the `tmp/` subtree excepted. That is exactly the property the
semantic goal needs, and no more. The refusal is asserted over the grant
filenames in `the_file_tools_cannot_address_a_permission_grant_file`, because it
is the one thing here that could regress in silence.

**Why seed on migration rather than start empty.** Starting every workspace
empty is maximally safe and silently breaks every working setup on upgrade. A
user who granted Slack a month ago would get a prompt with no explanation, which
trains them to click through prompts without reading. That is worse than the bug
being fixed. An upgrade preserves what the user already has, because taking a
grant away is a change they did not ask for and cannot see coming.

**Why only workspaces that already exist.** Seeding forever would reproduce the
original bug with extra steps, propagating `Bash`, `Python` and the host grant
into every workspace created from now on. The split between upgrade and creation
is the whole fix, and it needs no judgment about which workspace deserves which
grant. That judgment is not available to code anyway: which workspace a grant
was earned in is knowable by looking at one machine and not knowable here.

**Why a stamp rather than a content comparison.** A user who deletes a seeded
line has made a decision. A content comparison would read the absent line as
"not migrated yet" and put it back on the next boot. That is a permission the
user removed coming back. The per-workspace stamp is claimed before anything is
copied, so a crash mid-copy leaves that workspace empty and the user re-approves
once. Losing a grant costs one prompt; resurrecting one is a lie about consent.

**Why the registry for discovery.** Globbing `~/workspaces` is wrong on the
shape that matters most: a packaged install keeps its workspaces under
`~/Library/Application Support/com.lucidos.app/workspaces`. The gateway registry
is the list, and the engine mirrors the fields it needs rather than linking the
gateway crate (ADR 0014 §1). An unparseable registry seeds nothing and writes no
record. Reading it as "no workspaces" would close the door permanently on an
install whose grants were never copied.

**Why the migration lives in the engine.** The engine runs in every topology:
the packaged gateway spawns it, the dev launcher starts it, and the e2e suite
starts it with no gateway at all. Every grant-file concern already lives there.

## Consequences

- **Grants are not backed up**, because `is_excluded_workspace_path` excludes
  everything under `.lucidos`, and restore additionally drops the user dir. A
  restored workspace starts with no grants and asks on first use. Accepted and
  correct: a restore onto another machine must not silently reinstate a yes.
- **Grants do not survive delete-and-recreate** of a same-named workspace. Also
  correct, and it is the property the confused-deputy case wanted.
- **A workspace created tomorrow starts empty**, so the first gated command,
  coding-agent tool call or MCP call in it shows a card. That is the intended
  behaviour, not a regression, and the machine record on disk says why.
- **The originals stay in `~/.lucidos/`, unread.** A later release removes them.
  A migration that deletes the only copy of a permission set is not worth the
  disk it saves.
- **The Postgres role is still one role for every workspace database.** No
  `CREATE ROLE`, no `GRANT`, so every engine holds credentials that open every
  other workspace's database. Under this ADR's semantic goal that changes
  nothing, since `run_bash` reaches the same data with less effort. It is
  recorded because a future reader will assume it was handled. A role per
  workspace plus `REVOKE CONNECT` becomes worth doing the moment workspaces run
  under different uids, or a remote or multi-user deployment appears.
- **The per-server grant fingerprint shrinks.** Binding a grant to a server
  fingerprint was load-bearing because the grant file was machine-wide and a
  registry id is a per-workspace label. Per-workspace files close the
  cross-workspace half directly. The intra-workspace case survives: re-register
  a server against a different command and yesterday's grant still matches. So
  the fingerprint stays worth doing, as the smaller thing. See
  `artifacts/plans/2026-08-14-remote-mcp-transport-and-oauth.md` decision 12,
  whose ADR has not been written.

## Alternatives considered

**Leave commands machine-global.** The argument was that `Bash(git:*)` names its
own referent, so machine scope and grant meaning agree. True, and it loses: a
bare `Bash` grant is still a decision, and it was still made while looking at one
workspace. The live file held exactly that bare grant.

**`data/config/`.** Git-tracked and backed up, which would have kept grants
across a restore. Disqualified because the agent's file tools can write there.

**Seed every workspace forever.** Preserves everything and reproduces the bug.

**Start every workspace empty.** Safe, and it breaks every working setup on
upgrade with no explanation.

**Bump the gateway registry's `version` and migrate there.** The most idiomatic
option available: that mechanism exists to run a one-time thing once per install,
is atomic, and adds no new file. Rejected because it conflates a document-schema
version with a side effect on unrelated files. Regenerate or restore
gateway-data, and the version resets while the originals are still on disk. Every
workspace is then re-seeded, including ones created after the migration, which is
this bug returning through a side door.

**A marker in the gateway data dir.** Co-locates the record with the discovery
source. Rejected because gateway-data may not exist at all. It would need a
fallback to the user dir anyway, and two possible homes is worse than one. The
record is a fact about the three global files, and those live in the user dir on
every install.

**Stamp workspaces at creation instead of recording the migration.** Then
anything unstamped is pre-migration and gets seeded. Elegant, and it fails open:
any creation path that forgets to stamp (the dev launcher, restore from backup)
inherits every grant forever.
