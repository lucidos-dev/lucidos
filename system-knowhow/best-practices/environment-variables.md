---
name: Workspace File Conventions, Environment variables: Per-Workspace Config
description: Part of Workspace File Conventions. Its index lists the sections it covers.
---

One part of `system-knowhow/best-practices`, which lists every part.

## Environment variables: Per-Workspace Config

For environment that must be the **same for every subprocess in this workspace**, define an **environment variable** (Settings → System → Environment Variables). These are DB-backed, non-secret `NAME=value` pairs. The engine injects them as real env vars into every subprocess it spawns: `run_bash`, `run_python`, background tasks, scheduled scripts, triggers, and coding-agent (Claude Code / Codex) sessions.

- **You (the agent) can list, set and delete them** with the grouped `env_vars` tool (`action: list | set | delete`; `set`/`delete` take `name`, `set` also `value`). The retired `set_environment_variable` name still works as an alias for `set`. The user can also edit them in Settings. Changes take effect on the **next** tool call or agent turn, with **no engine restart**.
  - Exception: the engine's *own* shell-outs read their vars at startup. For example, the Apply-time `git push` uses `GIT_SSH_COMMAND` / `GH_CONFIG_DIR`. A change reaches the engine's own git on the next restart. Tool and agent subprocesses see it immediately.
- **Non-secret only.** Values appear in logs, the event store and tool-call payloads, by design. For API keys, tokens or passwords use a **credential** (`request_credential`). A call that names `CRED_<NAME>` gets it, and output masks its value.
- **Names** are uppercase letters, digits and underscores, not starting with a digit (e.g. `CLAUDE_CODE_USE_VERTEX`, `LUCIDOS_REPO`). Engine-owned names (`CRED_*`, `OAUTH_*`, `PATH`, internal `LUCIDOS_*` like `LUCIDOS_WORKSPACE` and `LUCIDOS_DATABASE_URL`) are rejected. Engine-owned vars always win a collision.
- **You cannot set a name that loads code**: `BASH_ENV`, `ENV`, `LD_*`, `DYLD_*`, `NODE_OPTIONS`, `PYTHONPATH`, `GIT_SSH*`, `GIT_CONFIG*`, `HOME`, `GH_CONFIG_DIR` and the other names `run_bash`'s command guard checks. A stored one would run in every later subprocess unchecked. Ask the user to set it in Settings.

The main use is **per-workspace identity**: a `gh` config dir, a Google `gws` config dir and project id, an SSH command. Then `gh` / `git push` from agent subprocesses authenticate as the right account:

```
GH_CONFIG_DIR=/Users/me/.config/gh-work
GIT_SSH_COMMAND=ssh -i /Users/me/.ssh/id_work -o IdentitiesOnly=yes
```

Setup is **interactive**. Both variables change what code `gh` and `git` run, so the user sets them and completes the auth handshake:

1. Pick a dedicated gh config dir and authenticate it once (user-run, opens a browser): `GH_CONFIG_DIR=<dir> gh auth login`.
2. For SSH push, make sure the key referenced by `GIT_SSH_COMMAND` is registered on that GitHub account.
3. Ask the user to set `GH_CONFIG_DIR` and `GIT_SSH_COMMAND` in Settings → System → Environment Variables. The `env_vars` tool refuses both.

**Want a credential's secret under a specific env var name?** Give the credential a custom env var name, for a CLI or SDK that expects an exact variable. Its secret then also injects as e.g. `GITHUB_TOKEN`, and the default `CRED_<NAME>` still works. A call gets it only when it names the alias or `CRED_<NAME>`. Set it two ways:

- In the credential editor (Settings → Accounts → Credentials).
- Up front, when the agent requests the credential. `request_credential` takes an optional `env_var_name` arg that pre-fills the modal's "Env var name" field. The user can still edit or clear it before saving.

The name must match `[A-Z_][A-Z0-9_]*` and can't be an engine-owned name (`CRED_*`, `OAUTH_*`, `PATH`, `LUCIDOS_*`). It works for single-value auth types only. A `password` credential ignores it, since it splits into `_USERNAME`/`_PASSWORD`.

**An auth handshake script never gets the custom name.** It receives only `CRED_*` and `OAUTH_*` names, so it reads `CRED_<NAME>`. See `building-an-auth-handshake`.

**A credential names every host it may be sent to.** Its Base URLs field is a set, one row per hostname, and the secret is refused at any host outside it. So a provider with API, clone and download traffic on different hostnames takes **one** credential naming all of them. Nothing is inferred from a hostname's spelling: name each in full. `request_credential` takes the whole set in `base_urls`. Asking for a host an existing credential does not cover reopens that credential, so the user never types the same secret twice.

**A secret can stay in 1Password.** Any secret field in the credential editor has a **From 1Password** toggle. On, the field takes a *secret reference* (`op://vault/item/field`) instead of the value. Lucidos reads it through the `op` utility with a service account token, set up under **Settings → Accounts → 1Password**. You cannot see or set the token.

Nothing changes for a script or app: it still names `CRED_<NAME>` or calls the proxy. When the value cannot be read, the call fails or the variable is left out, and the result says why. Point the user at 1Password or that Settings section, not at the script.

**Every secret the user enters is a credential.** A mailbox password is the `email_password` credential. A secret MCP server env value belongs in a credential too: `mcp(action="setup")` takes `{"credential": "<name>"}` as that variable's value. Removing the server deletes a credential named `mcp-<server id>-<VAR>` that no other server names. A credential with any other name stays.

The legacy `data/.env` file is retired. On the next engine startup, any existing `data/.env` moves into the environment-variables store and the file is removed.

## Key Rules

1. **Never nest artifacts**: `artifacts/artifacts/` is always wrong
2. **App data** → `artifacts/{app-id}/`
3. **App assets** → `apps/{id}/assets/`
4. **Imported data** → `artifacts/imported/{service}/`
5. **Generated content** → `artifacts/generated/` (or themed subfolder if a pattern emerges)
6. **Research** → `artifacts/research/`
7. **One source of truth**: don't duplicate files across locations
8. **Import the minimum.** Never dump a whole repo, dataset or archive into `artifacts/imported/` to grab one or two files. Artifact count is a performance axis: every extra file inflates linkify, file lists, scans and per-render paths.
   - **Cloning a repo to inspect, run or extract from it** → clone into `.lucidos/tmp/{repo-name}/` (ephemeral, gitignored, not counted). Inspect it with `read_file`. Then `copy_file` only the files the app needs into `artifacts/imported/{service}/`; it reads a `.lucidos/tmp/` source directly. If the user wants the full repo to persist (e.g. to keep editing it), ASK first where to put it. Never decide on your own to dump it under `artifacts/`.
   - **Bulk datasets and archives** (Wikifonia-style: thousands of files, of which you use a few) → same rule. Inspect under `.lucidos/tmp/` and extract the entries the app uses into `artifacts/imported/{service}/`. Leave the bulk archive out unless the user says "keep the whole archive available".
   - **Bulk reference corpora the user wants to keep, but not in the workspace** → `~/.lucidos/data/{name}/` (sibling to `~/.lucidos/knowhow/`, cross-workspace, persistent, agent-discoverable). Pin the absolute path in the app's knowhow so converter scripts find it. `lucidos data-store add {name} {source-dir}` moves an existing directory there.
   - **Intermediate, debug or one-shot render output** (e.g. cropping tiles, OMR debug pixmaps, scratch PNGs from a one-time analysis) never goes in `artifacts/imported/`. Use `.lucidos/tmp/` and delete it after the analysis.
   - **Inherited cruft from earlier sessions**: unexplained files under `data/artifacts/imported/` that the consuming app's source, scripts and knowhow never mention. Verify each one by grep, and ask the user about ambiguous cases. Then `git rm` the dead files in one commit, with before/after artifact counts in the message.
9. **Chained edits use post-edit content.** When you `edit_file` the same file twice in one turn, build the second `old_string` from what the first call returned, not from an earlier read. Otherwise the call fails with "The file was likely modified by a previous edit". A file needing 3+ edits in one turn takes one `write_file` rewrite instead. Every text-mode edit answers `N of M occurrences replaced`: read both numbers. `N < M` leaves the rest in place, which is right only if you meant the first one; `replace_all: true` takes the rest.
10. **Where the file tools can reach.** Three prefixes resolve, plus a registered repository by name. Everything else is rewritten or refused:
    - `data/` (the default): an untyped path like `notes.md` becomes `artifacts/notes.md`. Readable and writable.
    - `.lucidos/tmp/…`: the ephemeral scratch tree, workspace-root-relative, gitignored. **Readable** by `read_file` and as a `copy_file` source. That is how you pull a file out of a `git_clone` tmp checkout, or read what `http_request(temp_path)` saved. **Not writable**: the file tools git-commit everything they write, so `write_file` / `edit_file` / `delete_file` refuse it. Create scratch with `run_python` (cwd is the workspace root), delete it with `run_bash`.
    - `system-knowhow/…`: engine-shipped reference docs. Read-only.
    - A **registered repository**, via the `repo` argument on `read_file`, `glob_files`, `grep_files` and `edit_file`. Pass its name or id (`manage_repositories` action `list`), and the path or pattern becomes repo-root-relative, over the working tree. This is how you work on code. Never `cd` into a checkout to `cat`, `sed`, `grep` or `awk` it: that returns raw untruncated bytes and walks `target/`. The file tools chunk, slice by line, cap the match set, and see only what git sees. Gitignored files stay invisible, so a build log still needs `run_bash`.

    **Editing a repository commits nothing.** `edit_file` needs `commit: false` there, and `write_file` / `delete_file` / `copy_file` refuse `repo` outright. The edit lands only in the working tree. Say so when you report it, and point the user at `git diff`. Use `run_coding_agent` instead when the work adds or deletes files, or should arrive as a change the user reviews and applies.

    Everything else under `.lucidos/` is engine runtime state (`worktrees/`, `exhaust/`, `engine.pid`) and is refused in both directions. `..` and absolute paths are rejected outright. For a path outside the workspace that is not a registered repository, use `run_bash` (`sed -n`, `cat`).

## Images posted in a thread

Every image a user pastes (and every image you generate) has two addresses. `img-<hex>` is its **image handle**: content-derived, and stable for as long as the image exists. `thread:N` is its position, counted 1-based across the conversation. On upload, a fast vision model describes each one, and that description is kept as a derived fact.

- **Prefer the handle, and note it if you may want the image later.** `thread:N` renumbers whenever an earlier image turns up, so a noted number can end up naming a different picture. Every tool that takes an image reference accepts both forms.
- **Copy the handle, never count.** An image attached to the current message carries its handle in its label, e.g. `[1 image attached to this message (img-0123456789abcdef)]`. Pass that handle; do not work out its `thread:N`.
- **Recent images are already in your vision**: you can see and describe them natively, so just answer.
- **Older images age out of your vision** after a few newer messages, so stale screenshots don't mislead you in long threads. The history then shows only a text note like `[attached image (thread:2, img-0123456789abcdef, 2h ago, image not included, may be outdated)]` plus the saved description.
- **To see an aged-out image again, call `view_image`** with its handle, e.g. `view_image(image: "img-0123456789abcdef")`. It reloads the actual pixels into your vision. **When the user refers to an earlier image you cannot see, call `view_image`.** Never reply that there is no image, and never ask them to re-send it. Take the address from the history's image notes.
- **`[Image not sent: <model> does not read images. …]` means this model is text-only.** Its vision flag is off, so no image reaches it, `view_image` included. Answer from the saved description. If that is not enough, say so, and point the user at the model's vision flag in Settings → Models.
- **To keep an image** as a file, use `save_thread_image(image: "img-<hex>", path: "artifacts/...")` (git-committed). **To edit or restyle an image**, use `generate_image` with `input_images: ["img-<hex>"]`. To view an image *file* already under `data/artifacts/`, use `read_file`, not `view_image`.
