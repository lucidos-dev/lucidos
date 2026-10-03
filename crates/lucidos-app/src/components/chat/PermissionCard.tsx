import type { ComponentChildren } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { showToast } from '../../store/store';
import { resolveCodingAgentPermission, resolveCommandPermission, resolveMcpPermission } from '../../store/actions/permissions';
import { changeKindName, type AllowScope } from '../../store/thread-events';
import { errorDetail } from '../../utils/errorDetail';
import { CHOICE_CARD_ROLE, handAnsweredCardFocusToPrompt, handleChoiceCardKeyDown, seedChoiceCardFocus } from './choiceCardNav';
import { pendingDecisions } from './pendingDecisions';
import { followResolvedPermission } from './scrollState';

interface PermissionEvent {
  request_id: string;
  tool_use_id: string;
  tool_name: string;
  input: Record<string, unknown>;
  summary: string;
}

interface PermissionBodyProps {
  event: PermissionEvent;
  resolved?: { allowed: boolean; reason?: string; persist_scope?: AllowScope };
  /** Surrounding response was canceled / aborted / failed / superseded
   *  without a resolution landing — render every button disabled. */
  terminated?: boolean;
}

/** What a Codex change `kind` means in a SENTENCE. Anything unrecognized reads
 *  "change", which is true of every kind and claims nothing extra.
 *
 *  The dual `{type: "add"}` / bare-`"add"` shape is read by `changeKindName`, so
 *  the two surfaces keyed off a change kind share one reader. They deliberately
 *  do NOT share the verbs: this card says "wants to create /path", where a
 *  transcript step row says "Write foo.ts" because it has to read like the
 *  Claude Code row carrying the same edit (`CODEX_CHANGE_VERBS` in
 *  `store/thread-events/exchange.ts`).
 *
 *  A `Map`, not an object literal: the key is a string codex chose, and a plain
 *  object answers `constructor` / `toString` / `valueOf` / `__proto__` off its
 *  prototype. `CHANGE_VERBS['constructor'] ?? 'change'` returns the `Object`
 *  FUNCTION, not the default, so the `: string` below would be a lie and a
 *  function would be rendered into the card. A `Map` only ever answers what was
 *  put in it. */
const CHANGE_VERBS: ReadonlyMap<string, string> = new Map([
  ['add', 'create'],
  ['update', 'change'],
  ['delete', 'delete'],
]);
const DEFAULT_CHANGE_VERB = 'change';

function changeVerb(kind: unknown): string {
  return CHANGE_VERBS.get(changeKindName(kind)) ?? DEFAULT_CHANGE_VERB;
}

/** The files a Codex `file_change` approval is about, as `{verb, path}` pairs.
 *  The approval request itself carries no paths (only a nullable `reason` and
 *  `grantRoot`, both null in practice). The engine's app-server driver copies
 *  them across from the item's `item/started`, which codex emits first. Empty
 *  when it could not, which is the degrade case `renderFileChangeQuestion`
 *  handles. */
function fileChanges(input: Record<string, unknown>): { verb: string; path: string }[] {
  const raw = input.changes;
  if (!Array.isArray(raw) || raw.length === 0) return [];
  const changes: { verb: string; path: string }[] = [];
  for (const entry of raw) {
    const path = entry && typeof entry === 'object' ? (entry as { path?: unknown }).path : null;
    // One entry we cannot read discards the WHOLE set (mirroring the engine's
    // `FileTargets::Unresolved`). Listing only the files that parsed would show
    // a complete-looking card for a patch whose unnamed half writes elsewhere,
    // which is worse than the honest "wants to change files" degrade.
    if (typeof path !== 'string' || !path) return [];
    changes.push({ verb: changeVerb((entry as { kind?: unknown }).kind), path });
  }
  return changes;
}

/** The card for a Codex out-of-sandbox patch. Says what is happening to which
 *  files, because the alternative the user actually saw was a card reading
 *  "wants to use the file_change tool. Allow?" with nothing else on it. */
function renderFileChangeQuestion(input: Record<string, unknown>) {
  const changes = fileChanges(input);
  if (changes.length === 0) {
    // Nothing was announced for this item. Say the least that is still true,
    // and pass codex's own explanation through when it sent one.
    const reason = inputText(input, 'reason');
    return (
      <>
        The coding agent wants to change files{reason ? `: ${reason}` : ''}. Allow?
      </>
    );
  }
  if (changes.length === 1) {
    return (
      <>
        The coding agent wants to {changes[0].verb} <code>{changes[0].path}</code>. Allow?
      </>
    );
  }
  // One verb for the whole set only when they agree; a mixed patch is "change".
  const verb = changes.every(c => c.verb === changes[0].verb) ? changes[0].verb : DEFAULT_CHANGE_VERB;
  return (
    <>
      The coding agent wants to {verb} {changes.length} files. Allow?
      {/* Keyed by index, not by path: a patch may touch the same path twice
          (an update plus a move), and this list is static, so an index is both
          unique and stable where the path is only the latter. */}
      <ul class="permission-file-list">
        {changes.map((c, i) => (
          <li key={i}>
            <span class="permission-file-verb">{c.verb}</span> <code>{c.path}</code>
          </li>
        ))}
      </ul>
    </>
  );
}

/** A trimmed string field of a tool input, or `''`. */
function inputText(input: Record<string, unknown>, key: string): string {
  const value = input[key];
  return typeof value === 'string' ? value.trim() : '';
}

/** The command a card asks about, shown whole in a capped, scrolling block.
 *  Never truncated: the ask above it is the agent's claim, and this is what
 *  actually runs. */
function CommandBlock({ command }: { command: string }) {
  return <pre class="permission-command"><code>{command}</code></pre>;
}

/** The card for a shell command. The agent's own words lead (Claude Code's
 *  `description`, Codex's `reason`), because that is the question the user is
 *  answering. The command sits under it for the user to check. */
function renderCommandAsk(ask: string, command: string) {
  return (
    <>
      {ask || 'The coding agent wants to run this command.'}
      <CommandBlock command={command} />
    </>
  );
}

/** Frame the prompt around the tool name itself ("the **Edit** tool on `/path`")
 *  rather than burying it as a flat prefix in the summary. The original wording
 *  ("Claude Code wants to use Edit /path") read like a sentence about an action
 *  on a path; users didn't realise "Edit" was the tool whose permission they
 *  were about to grant. The subject is "the coding agent" — the same card is
 *  raised by Claude Code's MCP permission prompt AND the Codex app-server
 *  approval bridge, so naming Claude Code here would misattribute a Codex
 *  escalation at the exact moment the user is making a security decision.
 *
 *  **The two Codex tools are the exception and get their own sentence.**
 *  `Edit` / `Bash` / `Skill` are names a user meets elsewhere, so naming the
 *  tool orients them. `file_change` and `command_execution` are app-server wire
 *  identifiers that surface nowhere else, so the same framing produced "wants
 *  to use the file_change tool. Allow?": a security decision phrased in
 *  protocol jargon, about files it did not name. Those two say what the agent
 *  wants to DO instead.
 *
 *  **Shell commands lead with the agent's words**, see `renderCommandAsk`. */
export function renderQuestion(
  toolName: string,
  summary: string,
  input: Record<string, unknown> = {},
) {
  if (toolName === 'file_change') return renderFileChangeQuestion(input);
  // Untrimmed: the block shows exactly the bytes that will run.
  const command = typeof input.command === 'string' && input.command.trim() ? input.command : '';
  if (toolName === 'command_execution') {
    return command
      ? renderCommandAsk(inputText(input, 'reason'), command)
      : <>The coding agent wants to run a command. Allow?</>;
  }
  if (toolName === 'Bash' && command) {
    return renderCommandAsk(inputText(input, 'description'), command);
  }
  const space = summary.indexOf(' ');
  const arg = space === -1 ? null : summary.slice(space + 1);
  return arg ? (
    <>The coding agent wants to {toolAction(toolName, true)} <code>{arg}</code>. Allow?</>
  ) : (
    <>The coding agent wants to {toolAction(toolName, false)}. Allow?</>
  );
}

/** What using a tool does, as the verb phrase after "wants to": the form that
 *  takes the tool's argument, then the form that stands alone. A raw tool name
 *  is the agent's vocabulary, not the reader's. */
const TOOL_ACTIONS: Record<string, [string, string]> = {
  ExitPlanMode: ['stop planning and start work on its plan', 'stop planning and start work on its plan'],
  Read: ['read', 'read a file'],
  Edit: ['edit', 'edit a file'],
  MultiEdit: ['edit', 'edit a file'],
  Write: ['write', 'write a file'],
  NotebookEdit: ['edit the notebook', 'edit a notebook'],
  Glob: ['list files matching', 'list files'],
  Grep: ['search files for', 'search files'],
  WebFetch: ['fetch the web page', 'fetch a web page'],
  WebSearch: ['search the web for', 'search the web'],
  Skill: ['use the skill', 'use a skill'],
  Task: ['hand a task to a helper agent:', 'hand a task to a helper agent'],
  Agent: ['hand a task to a helper agent:', 'hand a task to a helper agent'],
};

function toolAction(toolName: string, withArg: boolean): ComponentChildren {
  const known = TOOL_ACTIONS[toolName];
  if (known) return withArg ? known[0] : known[1];
  const on = withArg ? ' on' : '';
  const mcp = /^mcp__(.+?)__(.+)$/.exec(toolName);
  if (mcp) return <>use the <strong>{mcp[2]}</strong> tool from <strong>{mcp[1]}</strong>{on}</>;
  return <>use the <strong>{toolName}</strong> tool{on}</>;
}

/** An "Always allow" grant in words: `Bash(git:*)` reads "git commands". The
 *  pattern is the allowlist's syntax, which the reader never types. */
export function grantLabel(pattern: string): ComponentChildren {
  const m = /^(\w+)\((.+?):\*\)$/.exec(pattern);
  if (m?.[1] === 'Bash') return <>Always allow <code>{m[2]}</code> commands</>;
  if (m?.[1] === 'Skill') return <>Always allow skills from <code>{m[2]}</code></>;
  return <>Always allow <code>{pattern}</code></>;
}

/** `grantLabel` as plain text, for an accessible name. */
export function grantText(pattern: string): string {
  const m = /^(\w+)\((.+?):\*\)$/.exec(pattern);
  if (m?.[1] === 'Bash') return `Always allow ${m[2]} commands`;
  if (m?.[1] === 'Skill') return `Always allow skills from ${m[2]}`;
  return `Always allow ${pattern}`;
}

/** A coding-agent tool named for a sentence: a tool-server tool by its own
 *  name and its server's, never the joined `mcp__server__tool` id. */
function toolText(toolName: string): string {
  const mcp = /^mcp__(.+?)__(.+)$/.exec(toolName);
  return mcp ? `The ${mcp[2]} tool from ${mcp[1]}` : `The ${toolName} tool`;
}

/** Mirrors the engine's `derive_allow_pattern` for the narrow scope: returns
 *  the pattern that would be persisted if the user clicks "Always allow" with
 *  narrow scope, or null when the tool has no meaningful sub-scope (so the
 *  card hides the narrow button). Returns null also when the input
 *  references a CC-protected path — see `inputTouchesProtectedPath`. Keep in
 *  sync with `claude_code.rs`. */
export function narrowPattern(toolName: string, input: Record<string, unknown>): string | null {
  if (inputTouchesProtectedPath(toolName, input)) return null;
  if (toolName === 'Skill') {
    const skill = typeof input.skill === 'string' ? input.skill : null;
    if (!skill) return null;
    const plugin = skill.includes(':') ? skill.split(':', 1)[0] : skill;
    return plugin ? `Skill(${plugin}:*)` : null;
  }
  if (toolName === 'Bash') {
    const command = typeof input.command === 'string' ? input.command : null;
    if (!command) return null;
    // `commandHead`, not the raw first token. The engine derives the stored
    // pattern from the unwrapped inner script. A raw-token label therefore
    // names a different grant from the one the click persists.
    const head = commandHead(command);
    return head ? `Bash(${head}:*)` : null;
  }
  return null;
}

/** Substrings that mark a path CC treats specially for destructive Bash
 *  commands. Empirically (probed 2026-05-16): `Bash rm -rf .../.claude/...`
 *  surfaces a permission card even when bare `Bash` is in `--allowedTools`,
 *  so persisting a Broad (`Bash`) or Narrow (`Bash(rm:*)`) grant from that
 *  card lies about future suppression. The trailing `/` anchors to the
 *  actual directory — `.gitignore` / `.claude_backup` are unaffected.
 *  Mirror of `CC_PROTECTED_PATH_MARKERS` in `claude_code.rs`. */
const CC_PROTECTED_PATH_MARKERS: readonly string[] = ['.claude/', '.git/'];

/** True when a `Bash` command references a path CC keeps under special
 *  permission routing (`.claude/` or `.git/`). The card hides Broad
 *  ("Always allow") and Narrow ("Always allow Bash(rm:*)") in that case —
 *  those buttons would persist patterns CC ignores for the same path.
 *  Hiding them is not the only thing holding that line. The engine's gate
 *  derives the same `null` and refuses to honour a stored pattern here. So a
 *  broad `Bash` granted elsewhere still cards on these paths (ADR 0125).
 *  Session ("Allow for this thread") still works because the engine
 *  intercepts before CC's gate — and it now survives an engine restart, since
 *  the engine rehydrates the grant from the persisted resolution events.
 *  Restricted to Bash because that's the only tool we've empirically observed
 *  surfacing the card on these paths under the user's bare-allowlist setup
 *  (Read / Edit / cat all auto-approved silently), and because a command is
 *  deliberately excluded from the engine's in-worktree write fast path: it can
 *  do anything, so a `Bash` touching `.claude/` still asks even in-worktree.
 *  Mirror of `input_touches_protected_path` in `claude_code.rs`. */
export function inputTouchesProtectedPath(
  toolName: string,
  input: Record<string, unknown>,
): boolean {
  if (toolName !== 'Bash') return false;
  const command = input.command;
  return typeof command === 'string'
    && CC_PROTECTED_PATH_MARKERS.some(m => command.includes(m));
}

/** Path-tools where the session-allow scope is per-file: subsequent prompts
 *  for the same `file_path` (or `notebook_path`) auto-resolve regardless of
 *  the diff being applied. Mirrors the engine's `AllowScope::Session` branch
 *  in `derive_allow_pattern`. */
const SESSION_PATH_TOOLS: ReadonlySet<string> = new Set(['Edit', 'Write', 'NotebookEdit']);

/** Pull a short display label for the session-allow button. For path-tools
 *  we show the basename; for Bash we show the command's first token; for
 *  Skill the plugin slug. Returns null when the tool has no useful identifier
 *  in the input — the button label then falls back to the bare tool name. */
export function sessionLabel(toolName: string, input: Record<string, unknown>): string | null {
  if (SESSION_PATH_TOOLS.has(toolName)) {
    const key = toolName === 'NotebookEdit' ? 'notebook_path' : 'file_path';
    const path = typeof input[key] === 'string' ? (input[key] as string) : null;
    if (!path) return null;
    const basename = path.split('/').filter(Boolean).pop();
    return basename || path;
  }
  if (toolName === 'Bash' || toolName === 'command_execution') {
    const command = typeof input.command === 'string' ? input.command : null;
    if (!command) return null;
    // Same reason as `narrowPattern`. Codex wraps every command it runs, so
    // the raw first token labelled every one of them with the same shell.
    const head = commandHead(command);
    return head ? `${head} …` : null;
  }
  if (toolName === 'Skill') {
    const skill = typeof input.skill === 'string' ? input.skill : null;
    if (!skill) return null;
    const plugin = skill.includes(':') ? skill.split(':', 1)[0] : skill;
    return plugin ? `${plugin}:*` : null;
  }
  return null;
}

/** Codex backend tool names (raised by the app-server approval bridge).
 *  Persisted scopes (Broad / Narrow) are meaningless for them — only Claude
 *  Code reads `cc-allowed-tools` — and `file_change` additionally derives no
 *  session pattern (see `SESSION_ALLOW_INEFFECTIVE`). Mirror of
 *  `CODEX_BACKEND_TOOLS` in `claude_code.rs`. */
const CODEX_BACKEND_TOOLS: ReadonlySet<string> = new Set(['command_execution', 'file_change']);

/** Tools whose "Allow for this thread" click would record nothing (the
 *  engine derives no session pattern) — the button is hidden so a click
 *  can't silently behave as allow-once. Mirror of the `file_change` arm in
 *  `derive_allow_pattern`'s Session branch.
 *
 *  `file_change` is a deliberate choice, not a data gap. Its approval input now
 *  carries the changed paths (the driver copies them off the item's
 *  `item/started`), so a per-file `file_change(<path>)` pattern would be as
 *  derivable as `Edit(<path>)`. Two reasons it still gets none: codex raises
 *  this approval only for a patch that escaped its sandbox, which is exactly
 *  the thing worth re-confirming each time, and a `changes` list names several
 *  files at once, so there is no single key a grant could stand for. */
const SESSION_ALLOW_INEFFECTIVE: ReadonlySet<string> = new Set(['file_change']);

/** Tools whose bare entry in `--allowedTools` cannot be respected by CC.
 *  Two reasons a tool ends up here:
 *    - Edit/Write/NotebookEdit: `--permission-mode acceptEdits` routes them
 *      through `--permission-prompt-tool` for CC's protected paths (`.claude/`,
 *      `.git/`) and auto-approves them everywhere else, so a bare `Edit` line
 *      in `cc-allowed-tools` never helps. (The engine now answers the
 *      in-worktree half of that itself — `cc_permission::worktree_write_auto_allowed`
 *      resolves an in-worktree file write with no card at all — so these tools
 *      reach a card only for a target OUTSIDE the worktree, or under its
 *      `.git/`. The allowlist entry is still ineffective for those.)
 *    - ExitPlanMode: CC always routes plan-mode exit through the permission
 *      prompt regardless of `--allowedTools`, because the plan must be
 *      reviewed by the user before the assistant continues.
 *  The "Always allow" broad button is hidden for these tools; users wanting
 *  in-thread persistence should use the session-allow button, which the engine
 *  intercepts before CC's gate. The engine's gate refuses a stored bare line
 *  for these tools too, through the same derivation (ADR 0125). See
 *  `inputTouchesProtectedPath` for the per-input variant of the same rule
 *  (Bash commands targeting protected paths). Keep in sync with
 *  `BROAD_ALLOW_INEFFECTIVE` in `claude_code.rs`. */
export const BROAD_ALLOW_INEFFECTIVE: ReadonlySet<string> = new Set([
  'Edit',
  'ExitPlanMode',
  'NotebookEdit',
  'Write',
]);

export type PermissionChoice = 'deny' | 'allow' | 'session' | 'narrow' | 'broad';

/** Which button a resolved card marks as picked, or `null` when nobody pressed
 *  one. A card the engine settled was not pressed: it expired, a restart
 *  closed it, or Lucidos allowed it on its own. So it marks none, and the note
 *  under the card says what happened. A verdict with no engine reason is a
 *  click, including an old refusal from before reasons, which reads as Deny. */
export function resolvedChoice(resolved: {
  allowed: boolean;
  persist_scope?: AllowScope;
  reason?: string;
}): PermissionChoice | null {
  if (engineResolutionNote(resolved)) return null;
  if (!resolved.allowed) return 'deny';
  return resolved.persist_scope ?? 'allow';
}

/** The reason a user's own Deny click carries. Mirrors `DENIAL_REASON` in the
 *  engine's `cc_permission.rs`, `command_permission.rs` and `mcp_permission.rs`. */
const USER_DENIAL_REASON = 'User denied';

/** The engine's resolution reasons, by their opening words, in plain words.
 *  The engine's own text is written for the agent, which reads it too, so the
 *  card translates rather than the engine rewording. Mirrors the `*_REASON`
 *  constants in `cc_permission.rs`, `command_permission.rs`,
 *  `mcp_permission.rs` and the orphan sweep in `agent_recovery/has_diff.rs`. */
export const PLAIN_RESOLUTION_NOTES: [string, string][] = [
  ['Interrupted by an engine restart', 'Closed when Lucidos restarted. Nobody denied it.'],
  ['Superseded by a new message', 'Closed because you sent a new message instead.'],
  ['Canceled by user', 'Closed because you stopped the reply.'],
  ['Coding agent session ended before answering', 'Closed because the agent finished its turn before anyone answered.'],
  ['Coding agent terminated before answering', 'Closed because the agent stopped before anyone answered.'],
  ['Auto-allowed: file write inside', 'Allowed by Lucidos: the agent was writing a file in its own working copy.'],
  ['Auto-allowed: the command guard', 'Allowed by Lucidos: its command check judged this safe to run.'],
  ['Auto-allowed: benign in-workspace operation', 'Allowed by Lucidos: a harmless step in a trigger run with nobody to ask.'],
  ['Auto-allowed: covered by the trigger', 'Allowed by Lucidos: the trigger was given permission for this kind of action.'],
  ['Auto-denied: catastrophic operation', 'Refused by Lucidos: this is never allowed in a run with nobody to ask.'],
  ['Auto-denied: this coding-agent session runs unattended', 'Refused by Lucidos: its command check could not confirm this was safe, and nobody was there to ask.'],
];

/** What a card the ENGINE resolved says under its buttons, in plain words.
 *  `null` for a user click, whose picked button says it all. A reason with no
 *  translation shows its first sentence, so a new one is never silent. */
export function engineResolutionNote(
  resolved: { allowed: boolean; reason?: string } | null | undefined,
): string | null {
  const reason = resolved?.reason?.trim();
  if (!reason || reason === USER_DENIAL_REASON) return null;
  const plain = PLAIN_RESOLUTION_NOTES.find(([opening]) => reason.startsWith(opening));
  if (plain) return plain[1];
  const end = reason.search(/\.\s/);
  return end === -1 ? reason : reason.slice(0, end + 1);
}

/** Per-button disabled + state-class. `answered` styling (picked / rejected)
 *  wins over `terminated` so the user's recorded decision stays visible even
 *  if a later abort lands. */
export function permissionButtonState({
  answered,
  terminated,
  isPicked,
}: {
  answered: boolean;
  terminated: boolean;
  isPicked: boolean;
}): { disabled: boolean; stateClass: string } {
  const disabled = answered || terminated;
  const stateClass = !answered ? ''
    : isPicked ? ' permission-btn-picked'
    : ' permission-btn-rejected';
  return { disabled, stateClass };
}

type ButtonSpec = {
  choice: PermissionChoice;
  btnClass: string;
  label: ComponentChildren;
  ariaLabel: string;
  onClick: () => void;
};

/** Which choice a live permission card seeds keyboard focus onto when it
 *  arrives (the *choice card* contract, see `choiceCardNav.ts`). `Deny` leads
 *  the button row, but a keyboard user answering a permission card
 *  overwhelmingly means to allow the one thing being asked about, so the seed
 *  is "Allow once" rather than the first button. That the seeded choice GRANTS
 *  is safe only because it is always ringed: `.permission-body[data-role=…]
 *  .action-btn:focus` shows the focus ring for a programmatic seed, so Enter's
 *  effect is on screen before it is pressed. Every card builder below includes
 *  an `allow` spec, which is what makes one constant enough for all three.
 *  Changing this value changes what a reflex Enter does to a permission
 *  request, so it is pinned by `permission-card.test.tsx`. */
export const DEFAULT_PERMISSION_CHOICE: PermissionChoice = 'allow';

/** Render one permission button with answered/terminated state styling. Shared
 *  by all three cards: coding-agent, command-guard and MCP. */
function renderPermissionButton(
  spec: ButtonSpec,
  state: { selected: PermissionChoice | null; answered: boolean; sending: boolean; terminated: boolean },
) {
  const isPicked = state.selected === spec.choice;
  const { disabled, stateClass } = permissionButtonState({
    answered: state.answered,
    terminated: state.terminated,
    isPicked,
  });
  return (
    <button
      type="button"
      class={`${spec.btnClass}${stateClass}`}
      onClick={disabled ? undefined : spec.onClick}
      disabled={disabled}
      aria-pressed={state.answered ? isPicked : undefined}
      aria-label={spec.ariaLabel}
      // Gated on `!disabled` so the marker never outlives the live card: an
      // answered / terminated card must not advertise a focus seed target.
      data-default-choice={spec.choice === DEFAULT_PERMISSION_CHOICE && !disabled ? 'true' : undefined}
    >
      {isPicked && (state.sending
        ? <span class="mini-spinner permission-btn-sending" aria-hidden="true" />
        : <span class="permission-btn-check" aria-hidden="true">✓ </span>)}
      {spec.label}
    </button>
  );
}

/** The shared card chrome: a question and one wrapping row of buttons. Each
 *  permission card (coding-agent, command-guard, MCP) builds its own `buttons`
 *  and feeds them here. */
function PermissionBodyShell({
  requestId,
  question,
  buttons,
  selected,
  answered,
  sending,
  terminated,
  note,
}: {
  requestId: string;
  question: ComponentChildren;
  buttons: ButtonSpec[];
  selected: PermissionChoice | null;
  answered: boolean;
  sending: boolean;
  terminated: boolean;
  note: string | null;
}) {
  const bodyStateClass = answered ? ' permission-body-answered'
    : terminated ? ' permission-body-terminated'
    : '';
  const state = { selected, answered, sending, terminated };
  // A live card is a *choice card* (see `choiceCardNav.ts`): arrows step across
  // its buttons in DOM order, and "Allow once" takes focus on arrival so Enter
  // resolves it. The marker and the
  // seed are both gated on live, so a resolved card is inert to the keyboard.
  // The seed is latched to the card's ARRIVAL inside `seedChoiceCardFocus`.
  // `live` is not a one-way flip: a failed resolve rolls the optimistic pending
  // back and the card returns to live, which without the latch would drag focus
  // to "Allow once" right after the user pressed Deny.
  const live = !answered && !terminated;
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (live) seedChoiceCardFocus(ref.current, requestId);
  }, [live, requestId]);
  return (
    <div
      class={`permission-body protected-surface${bodyStateClass}`}
      data-request-id={requestId}
      data-role={live ? CHOICE_CARD_ROLE : undefined}
      ref={ref}
      onKeyDown={live ? (e) => handleChoiceCardKeyDown(e, ref.current) : undefined}
    >
      <div class="permission-text">{question}</div>
      <div class="permission-actions">
        {buttons.map(spec => renderPermissionButton(spec, state))}
      </div>
      {note && <div class="permission-resolution-note">{note}</div>}
    </div>
  );
}

type PendingVerdict = { allowed: boolean; persist_scope?: AllowScope };

/** The three permission cards' optimistic picks, keyed by request id. The
 *  divider header reads it to say "Sending". */
export const pendingVerdicts = pendingDecisions<PendingVerdict>();

/** Optimistic decide helper shared by all three cards: stamp the pending choice,
 *  fire the resolve action, roll back + toast on failure. `resolve` is the
 *  permission action (`resolveCodingAgentPermission` / `resolveCommandPermission`
 *  / `resolveMcpPermission`), which posts the consent and nothing else. The
 *  pick stays pending until `resolved` arrives over SSE, which drains it. */
function usePermissionDecide(
  requestId: string,
  resolved: PendingVerdict | undefined,
  terminated: boolean | undefined,
  resolve: (id: string, allowed: boolean, persist?: AllowScope) => Promise<void>,
  broadGrant: () => string,
) {
  useEffect(() => {
    if (resolved) pendingVerdicts.clear(requestId);
  }, [resolved, requestId]);
  // A dead card draws no unconfirmed pick, matching its "Unresolved" header.
  // The pick stays stored, so a resolution that still lands shows it again.
  const pending = terminated ? undefined : pendingVerdicts.map.value.get(requestId);
  const sending = useDelayedFlag(!resolved && !!pending);
  const decide = async (allowed: boolean, persist?: AllowScope) => {
    handAnsweredCardFocusToPrompt();
    pendingVerdicts.set(requestId, { allowed, persist_scope: persist });
    // Deciding a card is a SUBMIT: the agent is expected to respond to it, so it
    // gets the same one reaction every other submit gets, anchored on this card's
    // own turn. All three permission-shaped cards decide through this hook, so
    // one call site serves them all. Before the awaited POST, because this is the
    // button's own tap and must not wait on the round trip. See `followSubmit`.
    followResolvedPermission(requestId);
    if (allowed && persist === 'broad') {
      // Coarse trust granted, so say exactly how much.
      showToast(`${broadGrant()} is now allowed without asking, in this workspace.`, 'info');
    }
    try {
      await resolve(requestId, allowed, persist);
    } catch (e) {
      pendingVerdicts.clear(requestId);
      showToast(`Could not send decision: ${errorDetail(e)}`, 'error');
    }
  };
  return { effective: resolved ?? pending, sending, decide };
}

/** Body of a `CodingAgentPermissionRequest` divider exchange — rendered inside
 *  the initiator panel which provides the chrome. `pending` is an optimistic
 *  override; SSE swaps in `resolved` once the paired
 *  `CodingAgentPermissionResolved` event arrives. */
export function PermissionBody({ event, resolved, terminated }: PermissionBodyProps) {
  const { effective, sending, decide } = usePermissionDecide(
    event.request_id,
    resolved,
    terminated,
    resolveCodingAgentPermission,
    () => toolText(event.tool_name),
  );

  const touchesProtected = inputTouchesProtectedPath(event.tool_name, event.input);
  const isCodexTool = CODEX_BACKEND_TOOLS.has(event.tool_name);
  const narrow = touchesProtected || isCodexTool
    ? null
    : narrowPattern(event.tool_name, event.input);
  const showBroad = !BROAD_ALLOW_INEFFECTIVE.has(event.tool_name)
    && !isCodexTool
    && !touchesProtected;
  const showSession = !SESSION_ALLOW_INEFFECTIVE.has(event.tool_name);
  const session = sessionLabel(event.tool_name, event.input);

  const selected = effective ? resolvedChoice(effective) : null;
  const answered = effective !== undefined;

  const buttons: ButtonSpec[] = [
    {
      choice: 'deny',
      btnClass: 'action-btn action-btn-danger',
      label: 'Deny',
      ariaLabel: 'Deny this permission request',
      onClick: () => void decide(false),
    },
    {
      choice: 'allow',
      btnClass: 'action-btn action-btn-confirm',
      label: 'Allow once',
      ariaLabel: 'Allow this permission request once',
      onClick: () => void decide(true),
    },
    ...(showSession ? [{
      choice: 'session' as const,
      btnClass: 'action-btn action-btn-secondary',
      label: 'Allow for this thread',
      ariaLabel: `Allow ${session ?? event.tool_name} for the rest of this thread`,
      onClick: () => void decide(true, 'session'),
    }] : []),
    ...(narrow ? [{
      choice: 'narrow' as const,
      btnClass: 'action-btn action-btn-secondary',
      label: grantLabel(narrow),
      ariaLabel: grantText(narrow),
      onClick: () => void decide(true, 'narrow'),
    }] : []),
    ...(showBroad ? [{
      choice: 'broad' as const,
      btnClass: 'action-btn action-btn-secondary',
      label: 'Always allow',
      ariaLabel: `Always allow ${event.tool_name}`,
      onClick: () => void decide(true, 'broad'),
    }] : []),
  ];

  return (
    <PermissionBodyShell
      requestId={event.request_id}
      question={renderQuestion(event.tool_name, event.summary, event.input)}
      buttons={buttons}
      selected={selected}
      answered={answered}
      sending={sending}
      terminated={!!terminated}
      note={engineResolutionNote(resolved)}
    />
  );
}

// ---------------------------------------------------------------------------
// Command-guard permission card (ADR 0002) — the chat mirror of the above.
// Different event shape (`command` text, not a structured tool `input`) and a
// different consent endpoint, but the same buttons + answered-state machinery.
// ---------------------------------------------------------------------------

interface CommandPermissionEvent {
  request_id: string;
  tool_use_id: string;
  tool_name: string;
  command: string;
  summary: string;
}

interface CommandPermissionBodyProps {
  event: CommandPermissionEvent;
  resolved?: { allowed: boolean; reason?: string; persist_scope?: AllowScope };
  terminated?: boolean;
}

const BASH_TOOLS: ReadonlySet<string> = new Set(['run_bash', 'run_bash_background']);

const GUARD_SHELLS: ReadonlySet<string> = new Set(['sh', 'bash', 'zsh', 'dash', 'ksh', 'ash']);

/** Basename of a head token, with grouping and quote characters stripped.
 *  Mirror of the engine's `normalized_head`. */
function normalizedHead(head: string): string {
  const unquoted = head.replace(/^[({\\]+/, '').replace(/^["']+|["']+$/g, '');
  return unquoted.split('/').pop() || unquoted;
}

/** A single-dash cluster of ASCII letters containing `c`. Mirror of the
 *  engine's `is_shell_c_flag`. */
function isShellCFlag(tok: string): boolean {
  if (tok.length < 2 || !tok.startsWith('-') || tok.startsWith('--')) return false;
  const letters = tok.slice(1);
  return letters.includes('c') && /^[A-Za-z]+$/.test(letters);
}

/** Whether the tail after a `-c` script operand can run something of its own.
 *  Mirror of the engine's `tail_runs_more_commands` plus
 *  `has_command_substitution`. */
function tailRunsMoreCommands(tail: string): boolean {
  return /[;|&\n><]/.test(tail) || tail.includes('$(') || tail.includes('`')
    || tail.includes('<(') || tail.includes('>(');
}

/** Unwrap one shell `-c` wrapper so the head comes from the inner script.
 *
 *  Mirror of the engine's `unwrap_shell_command`, and it has to stay one. The
 *  engine derives the pattern it STORES from the unwrapped text. A card reading
 *  the raw text therefore names a different grant from the one the click
 *  persists. Codex sends every command pre-wrapped, so that is the ordinary
 *  path rather than an edge case.
 *
 *  Returns the original command whenever the wrapper is not a plain one, which
 *  is the same conservative direction the engine takes. */
function unwrapShellCommand(command: string): string {
  const trimmed = command.trimStart();
  const first = trimmed.split(/\s+/).filter(Boolean)[0];
  if (!first || !GUARD_SHELLS.has(normalizedHead(first))) return command;

  // Walk EVERY whitespace-delimited token for the `-c` cluster, as the engine
  // does. Testing only the first dash token gave up on `bash -o pipefail -c
  // '…'`, so the card read `bash` while the click stored the inner head.
  const token = /\S+/g;
  let m: RegExpExecArray | null;
  while ((m = token.exec(trimmed)) !== null) {
    if (!isShellCFlag(m[0])) continue;
    const operand = trimmed.slice(m.index + m[0].length).trim();
    // The script is ONE word. Anything after it sets `$0` and the positional
    // parameters. A control operator there belongs to the OUTER shell and
    // unwrapping would hide it, so fall back to the whole command.
    const [script, tail] = splitShellScriptOperand(operand);
    if (tailRunsMoreCommands(tail)) return command;
    return script;
  }
  return command;
}

/** Characters that end an UNQUOTED shell word. */
const ENDS_SHELL_WORD = /[\s;|&<>()]/;

/** Take the `-c` script operand as POSIX builds it, and the tail after it.
 *
 *  A word JOINS adjacent quoted and unquoted runs, so the first close quote is
 *  not where it ends. `'rm -rf '\''/'\'''` is one word reading `rm -rf '/'`,
 *  which is exactly the idiom Codex emits. One quoting layer is decoded, since
 *  `-c` makes the inner shell re-parse the operand.
 *
 *  An operand not starting with a quote is returned whole, tail included, the
 *  same conservative direction the engine takes. */
function splitShellScriptOperand(s: string): [string, string] {
  if (s === '' || (s[0] !== "'" && s[0] !== '"')) return [s, ''];
  let word = '';
  let i = 0;
  while (i < s.length) {
    const c = s[i];
    if (c === "'") {
      const end = s.indexOf("'", i + 1);
      const stop = end === -1 ? s.length : end;
      word += s.slice(i + 1, stop);
      i = Math.min(stop + 1, s.length);
    } else if (c === '"') {
      i++;
      while (i < s.length && s[i] !== '"') {
        if (s[i] === '\\' && i + 1 < s.length && '"\\$`'.includes(s[i + 1])) {
          word += s[i + 1];
          i += 2;
          continue;
        }
        word += s[i];
        i++;
      }
      i = Math.min(i + 1, s.length);
    } else if (c === '\\') {
      i++;
      if (i < s.length) {
        word += s[i];
        i++;
      }
    } else if (opensSubstitution(s, i) || ENDS_SHELL_WORD.test(c)) {
      // A substitution glued to the word ends it HERE, so the opener lands in
      // the tail where `tailRunsMoreCommands` sees it. Otherwise the `$` joins
      // the script and the tail starts at `(`, which nothing recognises.
      break;
    } else {
      word += c;
      i++;
    }
  }
  return [word, s.slice(i)];
}

/** True when a substitution opener starts at `i`: `$(`, `<(`, `>(`, or a
 *  backtick. Mirror of the engine's `opens_substitution`. */
function opensSubstitution(s: string, i: number): boolean {
  return s[i] === '`' || ('$<>'.includes(s[i]) && s[i + 1] === '(');
}

/** If `tok` is an I/O redirection operator, whether its target is the NEXT
 *  token rather than glued onto this one. `null` when it is not a redirect.
 *  Mirror of the engine's `redirect_token_needs_target`: bash allows a
 *  redirect BEFORE the command, and reading it as the command word labels the
 *  card with a grant the engine never stores. */
function redirectTokenNeedsTarget(tok: string): boolean | null {
  const rest = tok.replace(/^[0-9&]+/, '');
  const op = /^(>>|>|<)/.exec(rest);
  if (!op) return null;
  return rest.length === op[1].length;
}

/** The command head shown on the Bash narrow / session buttons — mirror of the
 *  engine's `first_command_token`: unwrap a shell wrapper, skip privilege
 *  prefixes and `VAR=value` assignments, take the basename. `null` for an empty
 *  command. Pinned by `__tests__/permission-card-command-head.test.ts`. */
export function commandHead(command: string): string | null {
  const benign = new Set(['sudo', 'env', 'command', 'time', 'nice', 'builtin', 'exec']);
  const toks = unwrapShellCommand(command).trim().split(/\s+/).filter(Boolean);
  let i = 0;
  while (i < toks.length) {
    if (benign.has(toks[i]) || (!toks[i].startsWith('-') && toks[i].includes('='))) {
      i++;
      continue;
    }
    const needsTarget = redirectTokenNeedsTarget(toks[i]);
    if (needsTarget === null) break;
    i += needsTarget ? 2 : 1;
  }
  const head = toks[i];
  if (!head) return null;
  const base = head.split('/').filter(Boolean).pop();
  return base || null;
}

/** Body of a `CommandPermissionRequested` divider exchange. */
export function CommandPermissionBody({ event, resolved, terminated }: CommandPermissionBodyProps) {
  const { effective, sending, decide } = usePermissionDecide(
    event.request_id,
    resolved,
    terminated,
    resolveCommandPermission,
    () => (BASH_TOOLS.has(event.tool_name) ? 'Any shell command' : 'Any Python'),
  );

  const selected = effective ? resolvedChoice(effective) : null;
  const answered = effective !== undefined;

  const isBash = BASH_TOOLS.has(event.tool_name);
  const head = isBash ? commandHead(event.command) : null;
  // Bash gets a narrow `Bash(<head>:*)` sub-scope; Python is coarse (no narrow).
  const narrow = isBash && head ? `Bash(${head}:*)` : null;
  const sessionLabelText = isBash ? (head ? `${head} …` : null) : 'Python';

  const buttons: ButtonSpec[] = [
    {
      choice: 'deny',
      btnClass: 'action-btn action-btn-danger',
      label: 'Deny',
      ariaLabel: 'Deny running this command',
      onClick: () => void decide(false),
    },
    {
      choice: 'allow',
      btnClass: 'action-btn action-btn-confirm',
      label: 'Allow once',
      ariaLabel: 'Allow this command once',
      onClick: () => void decide(true),
    },
    {
      choice: 'session',
      btnClass: 'action-btn action-btn-secondary',
      label: 'Allow for this thread',
      ariaLabel: `Allow ${sessionLabelText ?? 'this command'} for the rest of this thread`,
      onClick: () => void decide(true, 'session'),
    },
    ...(narrow ? [{
      choice: 'narrow' as const,
      btnClass: 'action-btn action-btn-secondary',
      label: grantLabel(narrow),
      ariaLabel: grantText(narrow),
      onClick: () => void decide(true, 'narrow'),
    }] : []),
    {
      choice: 'broad',
      btnClass: 'action-btn action-btn-secondary',
      label: 'Always allow',
      ariaLabel: isBash ? 'Always allow any shell command' : 'Always allow any Python',
      onClick: () => void decide(true, 'broad'),
    },
  ];

  return (
    <PermissionBodyShell
      requestId={event.request_id}
      question={renderCommandQuestion(event.command, event.summary)}
      buttons={buttons}
      selected={selected}
      answered={answered}
      sending={sending}
      terminated={!!terminated}
      note={engineResolutionNote(resolved)}
    />
  );
}

/** The question for a command-guard card: the risk summary the guard produced,
 *  then the command in the same block the coding-agent card uses. */
export function renderCommandQuestion(command: string, summary: string) {
  return (
    <>
      The Lucidos Agent wants to run this command. {summary}
      <CommandBlock command={command} />
    </>
  );
}

// ---------------------------------------------------------------------------
// MCP permission card — the chat mirror of the above for MCP server tool calls.
// Different event shape (a server + tool identity and an args summary, not a
// command or a structured tool input) and a different consent endpoint, but the
// same buttons + answered-state machinery. The "Always allow" scopes persist to
// the workspace's mcp-allowed-tools: narrow → Mcp(server:tool), broad → Mcp(server:*).
// ---------------------------------------------------------------------------

interface McpPermissionEvent {
  request_id: string;
  tool_use_id: string;
  server_id: string;
  server_name: string;
  tool_name: string;
  arguments_summary: string;
}

interface McpPermissionBodyProps {
  event: McpPermissionEvent;
  resolved?: { allowed: boolean; reason?: string; persist_scope?: AllowScope };
  terminated?: boolean;
}

/** Body of an `McpPermissionRequested` divider exchange. */
export function McpPermissionBody({ event, resolved, terminated }: McpPermissionBodyProps) {
  const { effective, sending, decide } = usePermissionDecide(
    event.request_id,
    resolved,
    terminated,
    resolveMcpPermission,
    () => `Any tool on ${event.server_name}`,
  );

  const selected = effective ? resolvedChoice(effective) : null;
  const answered = effective !== undefined;

  const buttons: ButtonSpec[] = [
    {
      choice: 'deny',
      btnClass: 'action-btn action-btn-danger',
      label: 'Deny',
      ariaLabel: `Deny calling ${event.tool_name} on ${event.server_name}`,
      onClick: () => void decide(false),
    },
    {
      choice: 'allow',
      btnClass: 'action-btn action-btn-confirm',
      label: 'Allow once',
      ariaLabel: 'Allow this tool call once',
      onClick: () => void decide(true),
    },
    {
      choice: 'session',
      btnClass: 'action-btn action-btn-secondary',
      label: 'Allow for this thread',
      ariaLabel: `Allow ${event.tool_name} for the rest of this thread`,
      onClick: () => void decide(true, 'session'),
    },
    {
      choice: 'narrow',
      btnClass: 'action-btn action-btn-secondary',
      label: <>Always allow <code>{event.tool_name}</code></>,
      ariaLabel: `Always allow ${event.tool_name} on ${event.server_name}`,
      onClick: () => void decide(true, 'narrow'),
    },
    {
      choice: 'broad',
      btnClass: 'action-btn action-btn-secondary',
      label: <>Always allow <strong>{event.server_name}</strong></>,
      ariaLabel: `Always allow any tool on ${event.server_name}`,
      onClick: () => void decide(true, 'broad'),
    },
  ];

  return (
    <PermissionBodyShell
      requestId={event.request_id}
      question={renderMcpQuestion(event.server_name, event.tool_name, event.arguments_summary)}
      buttons={buttons}
      selected={selected}
      answered={answered}
      sending={sending}
      terminated={!!terminated}
      note={engineResolutionNote(resolved)}
    />
  );
}

/** The question line for an MCP permission card: which tool on which server,
 *  plus the (truncated) arguments the agent wants to call it with. */
export function renderMcpQuestion(serverName: string, toolName: string, argsSummary: string) {
  const trimmed = argsSummary.trim();
  // `{}` / empty args carry no signal — omit the args block so the card reads
  // cleanly for argument-less tool calls.
  const showArgs = trimmed.length > 0 && trimmed !== '{}';
  return (
    <>
      The Lucidos Agent wants to use the <strong>{toolName}</strong> tool from <strong>{serverName}</strong>
      {showArgs ? ', with exactly these inputs' : ''}. Allow?
      {showArgs && <pre class="permission-mcp-args"><code>{trimmed}</code></pre>}
    </>
  );
}
