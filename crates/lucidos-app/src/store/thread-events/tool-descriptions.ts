// The step labels a tool call draws. Only the transcript reads them, so this
// module stays out of the entry chunk (ADR 0288).
import { parseMcpToolName } from '../../utils/mcpToolName';

/** Format a multi-line code/command string as "Run <first line>" (truncated to 60 chars). */
function describeRun(text: string): string {
  for (const line of text.split('\n')) {
    const trimmed = line.trim();
    if (trimmed) return trimmed.length > 60 ? `Run ${trimmed.slice(0, 57)}...` : `Run ${trimmed}`;
  }
  return 'Run command';
}

/** Last path segment, or the whole string when it has none. */
function basename(p: string): string {
  return p.split('/').pop() || p;
}

/** Shell basenames that mark a `<shell> -c <script>` wrapper. Mirrors
 *  `WRAPPER_SHELLS` in `crates/lucidos-engine/src/core/mod.rs`, which is the
 *  authoritative list; `store/__tests__/wrapper-shells-mirror.test.ts` reads that
 *  source and fails if the two stop naming the same shells. */
export const WRAPPER_SHELLS = new Set(['sh', 'bash', 'zsh', 'dash', 'ksh', 'ash', 'fish']);

/** The script inside a login-shell wrapper, or `cmd` unchanged when there is no
 *  wrapper to see through.
 *
 *  Codex reports a shell step as the whole invocation its harness built,
 *  `/bin/zsh -lc "<script>"`, where Claude Code's `Bash` reports the script on
 *  its own. Left alone the two backends read as different tools on every row of
 *  a transcript, which is the thing this exists to stop.
 *
 *  TypeScript twin of Rust `shell_script_body` in
 *  `crates/lucidos-engine/src/core/mod.rs`. Rust is authoritative and writes
 *  the description stored on the event. This twin covers the step detail's
 *  un-elided value, which the client always derives, and events stored before
 *  descriptions existed. Both sides are asserted to agree in `thread-events-steps-status.test.ts`.
 *
 *  Only Codex's `command_execution` goes through here. A Claude Code session that
 *  literally invokes `bash -lc "..."` chose to, and its row must show it. */
export function shellScriptBody(cmd: string): string {
  const wrapper = /^\s*(\S+)\s+-([A-Za-z]+)\s+([\s\S]*)$/.exec(cmd);
  if (!wrapper) return cmd;
  const [, shell, letters, rest] = wrapper;
  // `-c`, `-lc`, `-ic`: one cluster of letters including the flag that says
  // "the next argument is the script".
  if (!WRAPPER_SHELLS.has(basename(shell)) || !letters.includes('c')) return cmd;
  const script = rest.trim();
  const quote = script[0];
  // A quoted script has to be exactly ONE quoted word. `zsh -lc "a" && "b"` is
  // two words and a pipeline, so no part of it is "the command". The label
  // then shows the invocation verbatim rather than a confident half of it.
  if (quote === "'" || quote === '"') return unquoteShellWord(script, quote) ?? cmd;
  // An UNQUOTED suffix is the script only when it is a single word. POSIX
  // `sh -c` reads ONE operand as the script and assigns the rest to `$0`, `$1`.
  // So `zsh -lc git status` runs `git` with `$0=status`, not `git status`. Reading it as the latter would put a command in the row that
  // never ran, which is the one thing this must never do.
  return script && !/\s/.test(script) ? script : cmd;
}

/** The contents of `s` when it is exactly one quoted shell word, unescaped for
 *  that quoting style. `null` when it is unterminated or carries anything after
 *  its closing quote. Twin of Rust `unquote_shell_word`. */
function unquoteShellWord(s: string, quote: string): string | null {
  let unescaped = '';
  let chunkStart = 1;
  let i = 1;
  while (i < s.length) {
    if (s[i] === quote) {
      // A single-quoted script cannot contain a quote, so the wrapper spells one
      // as close, escape, reopen. That sequence is not the terminator.
      if (quote === "'" && s.startsWith("'\\''", i)) {
        unescaped += `${s.slice(chunkStart, i)}'`;
        i += 4;
        chunkStart = i;
        continue;
      }
      if (i + 1 !== s.length) return null;
      return unescaped + s.slice(chunkStart, i);
    }
    // Inside double quotes a backslash escapes exactly four characters plus a
    // line continuation. Before anything else it is a literal backslash, and
    // the character after it is read normally.
    if (quote === '"' && s[i] === '\\' && i + 1 < s.length && '"\\$`\n'.includes(s[i + 1])) {
      unescaped += s.slice(chunkStart, i) + (s[i + 1] === '\n' ? '' : s[i + 1]);
      i += 2;
      chunkStart = i;
      continue;
    }
    i += 1;
  }
  return null; // unterminated
}

/** The name inside a Codex change `kind`. The app-server sends it as
 *  `{type: "add" | "update" | "delete"}`; older exec frames used a bare string,
 *  so both shapes resolve and anything else is `''`. Read in one place because
 *  two surfaces key off it: the step label below and `PermissionCard`'s approval
 *  sentence. */
export function changeKindName(kind: unknown): string {
  if (typeof kind === 'string') return kind;
  if (kind && typeof kind === 'object' && typeof (kind as { type?: unknown }).type === 'string') {
    return (kind as { type: string }).type;
  }
  return '';
}

/** The step-row verb each Codex change kind implies. Deliberately Claude Code's
 *  vocabulary rather than the sentence verbs `PermissionCard` uses ("wants to
 *  create /path"). That card builds a sentence. This is a label, and it has to
 *  read like the label a Claude Code row carrying the same edit would.
 *
 *  A `Map` for the same reason `CHANGE_VERBS` is one: the key is a string codex
 *  chose, and a plain object answers `constructor` off its prototype. */
const CODEX_CHANGE_VERBS: ReadonlyMap<string, string> = new Map([
  ['add', 'Write'],
  ['update', 'Edit'],
  ['delete', 'Delete'],
]);
/** True of every kind, so it claims nothing extra about one we can't read. */
const DEFAULT_CODEX_CHANGE_VERB = 'Change';

/** The `{verb, path}` pairs a Codex `file_change` call is about. Empty when the
 *  payload announces no changes. `path` is `''` for an entry that carries none. */
function codexFileChanges(args: unknown): { verb: string; path: string }[] {
  const raw = (args as { changes?: unknown } | null | undefined)?.changes;
  if (!Array.isArray(raw)) return [];
  return raw.map((entry) => {
    const e = (entry ?? {}) as { kind?: unknown; path?: unknown };
    return {
      verb: CODEX_CHANGE_VERBS.get(changeKindName(e.kind)) ?? DEFAULT_CODEX_CHANGE_VERB,
      path: typeof e.path === 'string' ? e.path : '',
    };
  });
}

/** Step label for a Codex `file_change`. Mirrors Rust's `file_change` arm: name
 *  the file when there is one. Claim a single verb only when every change in
 *  the patch agrees. `renderFileChangeQuestion` applies the same honesty rule
 *  to this payload on the permission card. */
function codexFileChangeLabel(args: unknown): string {
  const changes = codexFileChanges(args);
  if (changes.length === 0) return 'Apply file changes';
  const verb = changes.every(c => c.verb === changes[0].verb)
    ? changes[0].verb
    : DEFAULT_CODEX_CHANGE_VERB;
  if (changes.length === 1) {
    return changes[0].path ? `${verb} ${basename(changes[0].path)}` : `${verb} 1 file`;
  }
  return `${verb} ${changes.length} files`;
}

/** The bare tool name inside an `mcp__<server>__<tool>` identifier, or undefined
 *  when `name` is not shaped like one. Both backends name MCP tools this way,
 *  since Codex rebuilds its `mcp_tool_call` item into the same shape. So the
 *  server prefix is noise in a step row. Twin of Rust `mcp_tool_suffix`. */
function mcpToolSuffix(name: string): string | undefined {
  return parseMcpToolName(name)?.tool;
}

/** Full primary-arg value for an engine tool call, used as a hover tooltip when
 *  the rendered description elides it. Rust `describe_tool()` truncates
 *  commands, paths, prompts, and URLs to ~60 chars. Returns whichever single arg the
 *  description actually clips so the tooltip mirrors the un-elided form.
 *  Undefined when nothing useful would differ from the description. */
export function fullCommandForEngineTool(name: string, args: unknown): string | undefined {
  const a = args as Record<string, unknown> | null | undefined;
  if (!a) return undefined;
  const s = (k: string) => (typeof a[k] === 'string' ? (a[k] as string) : undefined);

  switch (name) {
    case 'run_bash': return s('command');
    case 'run_python': return s('code');
    case 'read_file':
    case 'write_file':
    case 'edit_file':
    case 'delete_file': return s('path');
    case 'copy_file': return s('destination');
    case 'import_file': return s('source_path');
    case 'browser_open':
    case 'http_request': return s('url');
    case 'web_search': return s('query');
    case 'execute_intent': return s('intent_id'); // Retired compatibility.
    case 'emit_event':
    case 'query_events': return s('event_type');
    case 'send_notification': return s('title');
    case 'send_email': return s('subject');
    case 'generate_image':
    case 'run_thread': return s('prompt');
    default: return undefined;
  }
}

/** Full primary-arg value for a Claude Code tool call, shown in the step detail
 *  (`StepDetailModal`'s `.step-detail-full`) when the rendered description
 *  elides it. Rust `describe_cc_tool()` in `crates/lucidos-engine/src/core/mod.rs`
 *  shows basenames for paths and only the URL origin for WebFetch. It also cuts
 *  Bash commands to their first line, at 57 chars. `Agent` returns `prompt`
 *  rather than the short `description` field Rust uses, since the prompt is the
 *  actual hidden detail. Undefined when no primary arg is defined for the tool. */
export function fullCommandForCCTool(name: string, args: unknown): string | undefined {
  const a = args as Record<string, unknown> | null | undefined;
  if (!a) return undefined;
  const s = (k: string) => (typeof a[k] === 'string' ? (a[k] as string) : undefined);

  switch (name) {
    case 'Read':
    case 'Edit':
    case 'MultiEdit':
    case 'Write':
    case 'NotebookEdit': return s('file_path');
    case 'Bash': return s('command');
    case 'WebFetch': return s('url');
    case 'Glob':
    case 'Grep': return s('pattern');
    case 'WebSearch': return s('query');
    case 'Agent': return s('prompt');
    case 'Skill': return s('skill');
    case 'TodoWrite': {
      const todos = a.todos;
      if (!Array.isArray(todos) || todos.length === 0) return undefined;
      const MARKERS: Record<string, string> = { completed: '[x]', in_progress: '[~]', pending: '[ ]' };
      return todos.map((t) => {
        const { content, activeForm, status } = (t ?? {}) as { content?: string; activeForm?: string; status?: string };
        const marker = MARKERS[status ?? ''] ?? '[?]';
        const text = (status === 'in_progress' && activeForm) ? activeForm : (content ?? '');
        return `${marker} ${text}`;
      }).join('\n');
    }
    // Codex's plan tool (both protocols normalize to {items: [{text, completed}]}
    // — see runtime/codex_parse.rs + codex_app_server_parse.rs). Same marker
    // list as CC's TodoWrite so the two backends' plan steps read alike.
    case 'todo_list': {
      const items = a.items;
      if (!Array.isArray(items) || items.length === 0) return undefined;
      return items.map((t) => {
        const { text, completed } = (t ?? {}) as { text?: string; completed?: boolean };
        return `${completed ? '[x]' : '[ ]'} ${text ?? ''}`;
      }).join('\n');
    }
    // Codex's other item types. Without these, no Codex step had an un-elided
    // value in its detail at all, where every Claude Code step does.
    case 'command_execution': {
      const cmd = s('command');
      return cmd ? shellScriptBody(cmd) : undefined;
    }
    case 'file_change': {
      const changes = codexFileChanges(a);
      // One entry we cannot name discards the whole set, mirroring
      // `PermissionCard`'s `fileChanges`: a complete-looking list whose unnamed
      // half writes elsewhere is worse than showing no list.
      if (changes.length === 0 || changes.some(c => !c.path)) return undefined;
      return changes.map(c => `${c.verb} ${c.path}`).join('\n');
    }
    case 'web_search': return s('query');
    default: return undefined;
  }
}

/** @deprecated Fallback for old events without a stored description. New descriptions come from Rust `describe_tool()`. */
export function describeEngineTool(name: string, args: unknown): string {
  const a = args as Record<string, unknown> | null | undefined;
  const str = (key: string) => (a && typeof a[key] === 'string' ? a[key] as string : '');

  switch (name) {
    case 'read_file': return str('path') ? `Read ${basename(str('path'))}` : 'Read file';
    case 'write_file': return str('path') ? `Write ${basename(str('path'))}` : 'Write file';
    case 'edit_file': return str('path') ? `Edit ${basename(str('path'))}` : 'Edit file';
    // `list_files` declares `"properties": {}` (`llm/tools/file.rs`), so it takes
    // no arguments at all and the old `path` lookup could never match. Rust's own
    // `describe_tool` renders it as a constant for the same reason.
    case 'list_files': return 'List files';
    case 'copy_file': return str('destination') ? `Copy to ${basename(str('destination'))}` : 'Copy file';
    case 'delete_file': return str('path') ? `Delete ${basename(str('path'))}` : 'Delete file';
    // `source_path` is `import_file`'s only argument (required, see
    // `llm/tools/file.rs`), and it is what Rust's own `describe_tool` and the
    // sibling `fullCommandForEngineTool` above both read. The payload has no
    // `url` key. An arm reading one would draw the bare "Import file", and the
    // row would disagree with its own hover tooltip.
    case 'import_file': return str('source_path') ? `Import ${basename(str('source_path'))}` : 'Import file';
    case 'run_bash': return str('command') ? describeRun(str('command')) : 'Run bash';
    case 'run_python': return str('code') ? describeRun(str('code')) : (str('description') || 'Run Python');
    case 'execute_intent': return str('intent_id') ? `Run intent: ${str('intent_id')}` : 'Run intent'; // Retired compatibility.
    case 'emit_event': return str('event_type') ? `Emit ${str('event_type')}` : 'Emit event';
    case 'query_events': return str('event_type') ? `Query ${str('event_type')}` : 'Query events';
    case 'web_search': return str('query') ? `Search "${str('query')}"` : 'Web search';
    case 'http_request': return str('url') ? `HTTP ${str('method') || 'GET'} ${str('url').split('/').slice(0, 3).join('/')}` : 'HTTP request';
    case 'send_notification': return str('title') ? `Notify: ${str('title')}` : 'Send notification';
    case 'send_email': return str('subject') ? `Email: ${str('subject')}` : 'Send email';
    case 'read_emails': return 'Read emails';
    case 'read_email': return 'Read email';
    // `topic` is `fetch_news`'s search term (required, see `llm/tools/web.rs`),
    // and it is what Rust's own `describe_tool` reads. There is no `query` key
    // on the payload.
    case 'fetch_news': return str('topic') ? `News: ${str('topic')}` : 'Fetch news';
    case 'browser_open': return str('url') ? `Open ${str('url').split('/').slice(0, 3).join('/')}` : 'Open browser';
    case 'browser_extract': return 'Extract page content';
    case 'browser_click': return str('selector') ? `Click ${str('selector')}` : 'Click element';
    case 'browser_type': return 'Type text';
    case 'browser_eval': return 'Run browser script';
    case 'browser_screenshot': return 'Take screenshot';
    case 'browser_close': return 'Close browser';
    case 'git_clone': return str('url') ? `Clone ${basename(str('url'))}` : 'Clone repo';
    case 'create_app': return str('name') ? `Create app: ${str('name')}` : 'Create app';
    case 'create_trigger': return str('name') ? `Schedule: ${str('name')}` : 'Create trigger';
    case 'run_claude': return 'Run Claude Code';
    // The read decision (ADR 0417): a no is quiet, a yes asks.
    case 'request_read': return a?.read === false ? 'No read needed' : 'Asking you to read this reply';
    case 'correct_memory': return 'Correct memory';
    case 'set_language': return str('language') ? `Set language: ${str('language')}` : 'Set language';
    case 'set_timezone': return str('timezone') ? `Set timezone: ${str('timezone')}` : 'Set timezone';
    case 'refresh_app': { const n = str('app_name') || str('app_id'); return n ? `Refresh ${n}` : 'Refresh app'; }
    case 'capture_app': { const n = str('app_name') || str('app_id'); return n ? `Capture ${n}` : 'Capture app'; }
    // `service_name` is what `request_credential` takes (required, see
    // `llm/tools/misc.rs`); `provider` belongs to `connect_oauth_account`.
    case 'request_credential': return str('service_name') ? `Request ${str('service_name')} credential` : 'Request credential';
    case 'configure_email': return 'Configure email';
    case 'connect_oauth_account': return str('provider') ? `Connect ${str('provider')}` : 'Connect account';
    case 'navigate_ui': {
      const target = str('target');
      if (target === 'app' || target === 'app-ui') { const n = str('app_name') || str('app_id'); return n ? `Open ${n}` : 'Open app'; }
      // `file_path` is `navigate_ui`'s file argument (see `get_navigate_ui_tool`
      // in `llm/tools/misc.rs`, and `handleNavigationRequest` which consumes the
      // same payload). There is no `path` key on it.
      if (target === 'file') return str('file_path') ? `Open ${basename(str('file_path'))}` : 'Open file';
      if (target === 'url') return str('url') ? `Open ${str('url').split('/').slice(0, 3).join('/')}` : 'Open URL';
      return target ? `Open ${target}` : 'Navigate UI';
    }
    case 'notifications':
    case 'read_notifications': {
      const a = str('action');
      if (a === 'mark_read') return 'Mark notification read';
      if (a === 'mark_all_read') return 'Mark all notifications read';
      return 'Read notifications';
    }
    // Grouped manifest tools (the flat per-verb cases above stay for aliases).
    case 'triggers': {
      const a = str('action');
      if (a === 'create') return str('name') ? `Schedule: ${str('name')}` : 'Create trigger';
      if (a === 'update') return str('name') ? `Update trigger: ${str('name')}` : 'Update trigger';
      if (a === 'delete') return 'Delete trigger';
      if (a === 'pause') return 'Pause trigger';
      if (a === 'resume') return 'Resume trigger';
      return 'List triggers';
    }
    case 'trigger_groups': {
      const a = str('action');
      if (a === 'create') return str('name') ? `Create group: ${str('name')}` : 'Create trigger group';
      if (a === 'rename') return str('name') ? `Rename group: ${str('name')}` : 'Rename trigger group';
      if (a === 'reorder') return 'Reorder trigger groups';
      if (a === 'delete') return 'Delete trigger group';
      return 'List trigger groups';
    }
    case 'preferences': {
      const a = str('action');
      if (a === 'set') return str('key') ? `Set ${str('key')}` : 'Set preference';
      return 'Read preferences';
    }
    case 'mcp': {
      const a = str('action');
      if (a === 'setup') return str('name') ? `Setup MCP: ${str('name')}` : 'Setup MCP server';
      if (a === 'enable') return str('id') ? `Enable MCP: ${str('id')}` : 'Enable MCP server';
      if (a === 'disable') return str('id') ? `Disable MCP: ${str('id')}` : 'Disable MCP server';
      if (a === 'start') return 'Start MCP server';
      if (a === 'stop') return 'Stop MCP server';
      if (a === 'remove') return 'Remove MCP server';
      return 'List MCP servers';
    }
    case 'plugins': {
      const a = str('action');
      if (a === 'install') return str('source') ? `Install plugin: ${basename(str('source'))}` : 'Install plugin';
      if (a === 'register_marketplace') return 'Register marketplace';
      if (a === 'update') return str('id') ? `Update plugin: ${str('id')}` : 'Update plugin';
      if (a === 'uninstall') return str('id') ? `Uninstall plugin: ${str('id')}` : 'Uninstall plugin';
      return 'Check plugin updates';
    }
    case 'events': {
      const a = str('action');
      if (a === 'emit') return str('event_type') ? `Emit ${str('event_type')}` : 'Emit event';
      if (a === 'count') return 'Count events';
      return str('event_type') ? `Query ${str('event_type')}` : 'Query events';
    }
    case 'changes': {
      const a = str('action');
      if (a === 'apply') return 'Apply change';
      return 'List changes';
    }
    case 'thread_queue': {
      const a = str('action');
      if (a === 'update_policy') return 'Update Thread Queue policy';
      if (a === 'run_now') return 'Run queued entry now';
      if (a === 'drop') return 'Drop queued entry';
      return 'List Thread Queue';
    }
    // `memory` and `threads` both gained READ actions, so an arm that ignores
    // `action` labels a lookup as a mutation: "Correct memory" for a search
    // tells the user their long-term memory was written to when it was only
    // read. Mirrors the same split in `core::mod::tool_label`.
    case 'memory': {
      const a = str('action');
      if (a === 'search') return 'Search memory';
      if (a === 'source') return 'Trace memory to its conversation';
      return 'Correct memory';
    }
    case 'threads': {
      const a = str('action');
      if (a === 'count') return 'Count threads';
      if (a === 'search') return 'Search past conversations';
      return 'List threads';
    }
    case 'enable_push_notifications': return 'Enable push notifications';
    case 'setup_mcp_server': return str('name') ? `Setup MCP: ${str('name')}` : 'Setup MCP server';
    case 'list_mcp_servers': return 'List MCP servers';
    // enable/disable/start/stop/remove address an already-registered server by `id` (only
    // `setup` takes a `name`). Both the `mcp` domain in `capability_manifest`
    // and the executor in `engine/tools/mcp.rs` read `id`.
    case 'start_mcp_server': return str('id') ? `Start MCP: ${str('id')}` : 'Start MCP server';
    case 'stop_mcp_server': return str('id') ? `Stop MCP: ${str('id')}` : 'Stop MCP server';
    case 'enable_mcp_server': return str('id') ? `Enable MCP: ${str('id')}` : 'Enable MCP server';
    case 'disable_mcp_server': return str('id') ? `Disable MCP: ${str('id')}` : 'Disable MCP server';
    case 'remove_mcp_server': return str('id') ? `Remove MCP: ${str('id')}` : 'Remove MCP server';
    case 'list_apps': return 'List apps';
    case 'list_triggers': return 'List triggers';
    case 'update_trigger': return str('name') ? `Update trigger: ${str('name')}` : 'Update trigger';
    case 'delete_trigger': return 'Delete trigger';
    case 'browser_forget_login': return 'Forget browser login';
    case 'browser_clear_data': return 'Clear browser data';
    case 'run_thread': return str('prompt') ? `Run thread: ${str('prompt').slice(0, 50)}` : 'Run thread';
    case 'generate_image': return str('prompt') ? `Generate image: ${str('prompt').slice(0, 44)}` : 'Generate image';
    case 'manage_repositories': return 'Manage repositories';
    default: { const s = name.replace(/_/g, ' '); return s.charAt(0).toUpperCase() + s.slice(1); }
  }
}

/** @deprecated Fallback for old events without a stored description. New descriptions come from Rust `describe_cc_tool()`. */
export function describeCCTool(name: string, args: unknown): string {
  const a = args as Record<string, unknown> | null | undefined;
  const str = (key: string) => (a && typeof a[key] === 'string' ? a[key] as string : '');

  switch (name) {
    case 'Read': return str('file_path') ? `Read ${basename(str('file_path'))}` : 'Read file';
    case 'Edit': return str('file_path') ? `Edit ${basename(str('file_path'))}` : 'Edit file';
    case 'Write': return str('file_path') ? `Write ${basename(str('file_path'))}` : 'Write file';
    case 'MultiEdit': return str('file_path') ? `Edit ${basename(str('file_path'))}` : 'Edit file';
    case 'Glob': return str('pattern') ? `Find ${str('pattern')}` : 'Find files';
    case 'Grep': return str('pattern') ? `Search '${str('pattern')}'` : 'Search code';
    case 'Bash': return str('command') ? describeRun(str('command')) : 'Run command';
    case 'WebFetch': return str('url') ? `Fetch ${str('url').split('/').slice(0, 3).join('/')}` : 'Fetch URL';
    case 'WebSearch': return str('query') ? `Search '${str('query')}'` : 'Web search';
    case 'Agent': return str('description') || 'Run agent';
    case 'Skill': return str('skill') ? `Run skill: ${str('skill')}` : 'Run skill';
    case 'NotebookEdit': return str('file_path') ? `Edit ${basename(str('file_path'))}` : 'Edit notebook';
    case 'TodoWrite': return 'Update plan';
    case 'ExitPlanMode': return 'Present plan for approval';
    // Codex item types (see `runtime/codex_parse.rs`): Codex reports
    // coarse-grained items, not named tools like CC. Each arm lands on the SAME
    // sentence its Claude Code counterpart above produces. The two backends
    // share every transcript component, so a row that reads differently is the
    // only thing left that can tell them apart.
    case 'command_execution': return describeRun(shellScriptBody(str('command')));
    case 'file_change': return codexFileChangeLabel(a);
    case 'web_search': return str('query') ? `Search '${str('query')}'` : 'Web search';
    case 'todo_list': return 'Update plan';
    default:
      // An MCP tool reaches both backends under the same name, and the server
      // prefix is noise. Without this the raw identifier IS the label.
      if (name.startsWith('mcp__')) return `MCP: ${mcpToolSuffix(name) ?? name}`;
      return name;
  }
}
