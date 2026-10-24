---
name: Workspace audit: plugins
description: The plugins audit section: the engine requirement on installed plugins and on plugin trees the workspace authors, and the setup_revision fields of those trees.
---

# Workspace audit section: plugins

One *audit section* of `system-knowhow/workspace-audit`. The root says how to
run it alone or in a full pass, how to merge its receipts, and how to write
the report. This file carries its scan, its checks and its fixes.

## Scan

Append these lines to the root's scan preamble, before its receipts tail:

```bash
plugin_manifests=""
while IFS= read -r -d '' m; do
  has_id=$(grep -qE '^id[[:space:]]*=' "$m" && echo y)
  has_version=$(grep -qE '^version[[:space:]]*=' "$m" && echo y)
  has_name=$(grep -qE '^name[[:space:]]*=' "$m" && echo y)
  has_desc=$(grep -qE '^description[[:space:]]*=' "$m" && echo y)
  if [ "$has_id" = y ] && [ "$has_version" = y ] && [ "$has_name" = y ] && [ "$has_desc" = y ]; then
    if grep -qE '^engine[[:space:]]*=' "$m"; then
      tag="engine declared"
    else
      tag="no engine"
    fi
    rev=$(sed -nE 's/^setup_revision[[:space:]]*=[[:space:]]*(.*)$/\1/p' "$m" | head -1)
    [ -n "$rev" ] && tag="$tag, setup_revision $rev"
    plugin_manifests="$plugin_manifests$m ($tag)
"
  fi
done < <(find . -name manifest.toml -not -path '*/node_modules/*' -print0 2>/dev/null)
n=$(printf '%s' "$plugin_manifests" | grep -c . || true)
printf '\n=== plugin-manifests (%s hits) ===\n%s\n' "$n" "${plugin_manifests:-none}"
receipts="$receipts| plugin-manifests | $n |\n"
```

## Checks

For each finding, capture: **location**, **what's wrong**, **which reference owns the rule** (link, don't quote), **suggested fix**.

Two surfaces, read differently. Per `system-knowhow/plugins.md` § "The `engine` requirement".

**a. Installed plugins: not a grep, a tool call.** Call `plugins(action="check_updates")` (no `id`) once, every pass, and add a `plugins-engine` row with the count checked to the receipts. Per installed plugin, it reports `installed_version`, `latest_version`, `changed`, `source`, and the **latest** version's `engine_requirement` / `engine_compatible` / `engine_incompatible_reason`.

`engine_requirement` describes the remote manifest.
`crates/lucidos-engine/src/engine/tools/plugins/registry.rs::check_plugin_updates_impl`
reads that freshly fetched manifest.

For the installed declaration, query `events` with `event_type="PluginInstalled"`.
Keep the latest per plugin id with no later `PluginUninstalled`.
Read `payload.data.manifest.manifest.engine`, as documented in
`system-knowhow/plugins.md` § `PluginInstalled`. Flag its absence on the installed
record, independently of the latest version's declaration.

For each installed plugin whose own `engine` is absent:

- A newer version exists, declares `engine`, and `engine_compatible` is true: finding. Fix: update the plugin (`plugins(action="update", id=...)`), which stages the confirm panel for the user to accept. Severity: **nit** (nothing is broken yet; the common case once an author adds a floor).
- A newer version exists but `engine_compatible` is false: finding, quoting `engine_incompatible_reason`. Fix: update Lucidos, then retry the plugin update. Severity: **nit**.
- The same `PluginInstalled` event has no `source` either (an archive install): finding. Fix: ask whoever shared the plugin for an updated archive that declares `engine`. Severity: **nit**. Check this before the `error` row below, where a sourceless plugin also shows up, less specifically.
- No newer version declares one (no update exists, or the latest manifest also lacks `engine`): finding. Fix: set `engine` to the first Lucidos release shipping every platform feature the plugin uses. `system-knowhow/plugins.md` § "The `engine` requirement" says how to find it. If `source` is a repo the user can push to, add the floor there; otherwise use "Proposing your patch upstream" in `system-knowhow/plugins.md`. Severity: **nit**.
- A `check_updates` entry carries any other `error` (a fetch failure, not the sourceless case): report "could not check `<id>`'s update, `<error text>`", never skip it. Severity: **smell** (the audit's coverage of this plugin is incomplete, not necessarily the plugin).

**Never edit an installed plugin's files under `data/` to add `engine`.** `manifest.toml` never lands under `data/`, and the floor is read from the fetched source. See `system-knowhow/plugins.md` § "The `engine` requirement".

**b. Plugin trees the workspace authors.** The `plugin-manifests` scan lists every `manifest.toml` under `data/` that has `id`, `version`, `name` and `description` (a plugin root). Each row says whether it declares `engine`, and shows its `setup_revision` when it has one:

- No `engine` key: finding. Fix: set `engine` to the first Lucidos release shipping every platform feature the plugin uses (`system-knowhow/plugins.md` § "The `engine` requirement" says how to find it). Bump `version` too, so existing installs receive the change. Severity: **nit**.
- An `engine` value that is not a valid semver requirement (not a string, empty, or a typo such as `"latest"`): read the file to confirm. Severity: **broken** (install refuses the plugin, so it cannot ship until fixed). Fix: same as above.
- A `setup_revision` that is not a positive integer (zero, negative, a string or a decimal), or a manifest with `setup_revision` and no non-empty `setup`: read the file to confirm. Severity: **broken** (install refuses the plugin). Fix: set a positive integer, or drop the field. `system-knowhow/plugins.md` § "A release can ask for setup to run again" owns the rule.
- A `setup_reason` that spans lines, passes 200 characters, or has no `setup_revision`: read the file to confirm. Severity: **broken**, for the same reason. Fix: shorten it to one line, or add the revision.

## Remediation

Fixes run only on request, as the root's § Remediation says. A fix gets the
table for its finding. None needs a coding agent: an update is a `plugins`
call, and setting `engine` in an authored tree is an inline edit.

### Asking plugins for an engine floor

| Old | New |
|---|---|
| Installed plugin has no `engine`; a compatible update exists | `plugins(action="update", id="<id>")`, which stages the confirm panel for the user to accept |
| Installed plugin has no `engine`; the available update needs a newer Lucidos | Tell the user to update Lucidos, quoting `engine_incompatible_reason`, then retry the plugin update |
| Installed plugin has no `engine`; no newer version declares one | Ask the plugin's author. Push the floor directly if `source` is a repo the user owns, else use `system-knowhow/plugins.md` § "Proposing your patch upstream" |
| Installed plugin has no `source` (archive install) | Ask whoever shared the plugin for an updated archive that declares `engine` |
| Authored plugin tree's `manifest.toml` has no `engine`, or an invalid one | Set `engine` to the plugin's floor release and bump `version`, in the author's own tree, never the installed copy |
| Authored plugin tree's `setup_revision` or `setup_reason` is malformed | Fix the field in the author's own tree, never the installed copy, and bump `version` |

- **Updates stage the confirm panel, never confirm themselves.** `plugins(action="update", ...)` opens the usual update panel. This audit turn stops there; the user clicks **Update**.
- **Never edit the installed copy** under `data/`, for the reason § Checks gives. Fix the plugin's own source tree, or propose the change upstream.
