import type { ComponentChildren } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
import { useSignal } from '@preact/signals';
import { showToast } from '../../store/store';
import { resolveCodingAgentPermission, resolveCommandPermission, resolveMcpPermission } from '../../store/actions/permissions';
import type { PersistScope } from '../../store/thread-events';
import { errorDetail } from '../../utils/errorDetail';
import { CHOICE_CARD_ROLE, handleChoiceCardKeyDown, seedChoiceCardFocus } from './choiceCardNav';

interface PermissionEvent {
  request_id: string;
  tool_use_id: string;
  tool_name: string;
  input: Record<string, unknown>;
  summary: string;
}

interface PermissionBodyProps {
  event: PermissionEvent;
  resolved?: { allowed: boolean; reason?: string; persist_scope?: PersistScope };
  /** Surrounding response was canceled / aborted / failed / superseded
   *  without a resolution landing — render every button disabled. */
  terminated?: boolean;
}

/** Frame the prompt around the tool name itself ("the **Edit** tool on `/path`")
 *  rather than burying it as a flat prefix in the summary. The original wording
 *  ("Claude Code wants to use Edit /path") read like a sentence about an action
 *  on a path; users didn't realise "Edit" was the tool whose permission they
 *  were about to grant. The subject is "the coding agent" — the same card is
 *  raised by Claude Code's MCP permission prompt AND the Codex app-server
 *  approval bridge, so naming Claude Code here would misattribute a Codex
 *  escalation at the exact moment the user is making a security decision. */
export function renderQuestion(toolName: string, summary: string) {
  const space = summary.indexOf(' ');
  const arg = space === -1 ? null : summary.slice(space + 1);
  return arg ? (
    <>
      The coding agent wants to use the <strong>{toolName}</strong> tool on <code>{arg}</code>. Allow?
    </>
  ) : (
    <>
      The coding agent wants to use the <strong>{toolName}</strong> tool. Allow?
    </>
  );
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
    const first = command.trim().split(/\s+/)[0];
    return first ? `Bash(${first}:*)` : null;
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
    const first = command.trim().split(/\s+/)[0];
    return first ? `${first} …` : null;
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
 *  session pattern (its input has no stable identifier, so a session grant
 *  would blanket-approve every future out-of-sandbox write). Mirror of
 *  `CODEX_BACKEND_TOOLS` in `claude_code.rs`. */
const CODEX_BACKEND_TOOLS: ReadonlySet<string> = new Set(['command_execution', 'file_change']);

/** Tools whose "Allow for this thread" click would record nothing (the
 *  engine derives no session pattern) — the button is hidden so a click
 *  can't silently behave as allow-once. Mirror of the `file_change` arm in
 *  `derive_allow_pattern`'s Session branch. */
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
 *  intercepts before CC's gate. See `inputTouchesProtectedPath` for the
 *  per-input variant of the same rule (Bash commands targeting protected
 *  paths). Keep in sync with `BROAD_ALLOW_INEFFECTIVE` in `claude_code.rs`. */
export const BROAD_ALLOW_INEFFECTIVE: ReadonlySet<string> = new Set([
  'Edit',
  'ExitPlanMode',
  'NotebookEdit',
  'Write',
]);

export type PermissionChoice = 'deny' | 'allow' | 'session' | 'narrow' | 'broad';

/** Recovery-emitted orphan resolutions arrive with `allowed: false` and no
 *  scope — those map to `'deny'`, which marks the Deny button as the
 *  surviving outcome even though the user never clicked. */
export function resolvedChoice(resolved: {
  allowed: boolean;
  persist_scope?: PersistScope;
}): PermissionChoice {
  if (!resolved.allowed) return 'deny';
  return resolved.persist_scope ?? 'allow';
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
  row: 'primary' | 'secondary';
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
 *  by both the coding-agent and command-guard cards. */
function renderPermissionButton(
  spec: ButtonSpec,
  state: { selected: PermissionChoice | null; answered: boolean; terminated: boolean },
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
      {isPicked && <span class="permission-btn-check" aria-hidden="true">✓ </span>}
      {spec.label}
    </button>
  );
}

/** The shared card chrome: a question line + a primary row + one row per
 *  secondary button. Both permission cards (coding-agent, command-guard) build
 *  their own `buttons` and feed them here. */
function PermissionBodyShell({
  requestId,
  question,
  buttons,
  selected,
  answered,
  terminated,
}: {
  requestId: string;
  question: ComponentChildren;
  buttons: ButtonSpec[];
  selected: PermissionChoice | null;
  answered: boolean;
  terminated: boolean;
}) {
  const primary = buttons.filter(b => b.row === 'primary');
  const secondary = buttons.filter(b => b.row === 'secondary');
  const bodyStateClass = answered ? ' permission-body-answered'
    : terminated ? ' permission-body-terminated'
    : '';
  const state = { selected, answered, terminated };
  // A live card is a *choice card* (see `choiceCardNav.ts`): arrows step across
  // its buttons (the primary row and every secondary row, in DOM order) and
  // "Allow once" takes focus on arrival so Enter resolves it. The marker and the
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
      class={`permission-body${bodyStateClass}`}
      data-request-id={requestId}
      data-role={live ? CHOICE_CARD_ROLE : undefined}
      ref={ref}
      onKeyDown={live ? (e) => handleChoiceCardKeyDown(e, ref.current) : undefined}
    >
      <div class="permission-text">{question}</div>
      <div class="permission-actions">
        {primary.map(spec => renderPermissionButton(spec, state))}
      </div>
      {secondary.map(spec => (
        <div key={spec.choice} class="permission-actions permission-actions-secondary">
          {renderPermissionButton(spec, state)}
        </div>
      ))}
    </div>
  );
}

/** Optimistic decide helper shared by both cards: stamp the pending choice,
 *  fire the resolve action, roll back + toast on failure. `resolve` is the
 *  permission action (`resolveCodingAgentPermission` / `resolveCommandPermission`),
 *  which force-scrolls to the bottom before POSTing consent so the agent's
 *  resumed stream tails. */
function usePermissionDecide(
  requestId: string,
  resolve: (id: string, allowed: boolean, persist?: PersistScope) => Promise<void>,
) {
  const pending = useSignal<{ allowed: boolean; persist_scope?: PersistScope } | null>(null);
  const decide = async (allowed: boolean, persist?: PersistScope) => {
    pending.value = { allowed, persist_scope: persist };
    if (allowed && persist === 'broad') {
      // Coarse trust granted — let the user feel the weight of it.
      showToast('You only live once', 'info');
    }
    try {
      await resolve(requestId, allowed, persist);
    } catch (e) {
      pending.value = null;
      showToast(`Could not send decision: ${errorDetail(e)}`, 'error');
    }
  };
  return { pending, decide };
}

/** Body of a `CodingAgentPermissionRequest` divider exchange — rendered inside
 *  the initiator panel which provides the chrome. `pending` is an optimistic
 *  override; SSE swaps in `resolved` once the paired
 *  `CodingAgentPermissionResolved` event arrives. */
export function PermissionBody({ event, resolved, terminated }: PermissionBodyProps) {
  const { pending, decide } = usePermissionDecide(event.request_id, resolveCodingAgentPermission);

  const effective = resolved ?? pending.value;

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
  const answered = selected !== null;

  const buttons: ButtonSpec[] = [
    {
      choice: 'deny',
      btnClass: 'action-btn action-btn-danger',
      label: 'Deny',
      ariaLabel: 'Deny this permission request',
      row: 'primary',
      onClick: () => void decide(false),
    },
    {
      choice: 'allow',
      btnClass: 'action-btn action-btn-confirm',
      label: 'Allow once',
      ariaLabel: 'Allow this permission request once',
      row: 'primary',
      onClick: () => void decide(true),
    },
    ...(showSession ? [{
      choice: 'session' as const,
      btnClass: 'action-btn action-btn-confirm',
      label: 'Allow for this thread',
      ariaLabel: `Allow ${session ?? event.tool_name} for the rest of this thread`,
      row: 'secondary' as const,
      onClick: () => void decide(true, 'session'),
    }] : []),
    ...(narrow ? [{
      choice: 'narrow' as const,
      btnClass: 'action-btn action-btn-confirm',
      label: <>Always allow <code>{narrow}</code></>,
      ariaLabel: `Always allow ${narrow}`,
      row: 'secondary' as const,
      onClick: () => void decide(true, 'narrow'),
    }] : []),
    ...(showBroad ? [{
      choice: 'broad' as const,
      btnClass: 'action-btn',
      label: 'Always allow',
      ariaLabel: `Always allow ${event.tool_name}`,
      row: 'secondary' as const,
      onClick: () => void decide(true, 'broad'),
    }] : []),
  ];

  return (
    <PermissionBodyShell
      requestId={event.request_id}
      question={renderQuestion(event.tool_name, event.summary)}
      buttons={buttons}
      selected={selected}
      answered={answered}
      terminated={!!terminated}
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
  resolved?: { allowed: boolean; reason?: string; persist_scope?: PersistScope };
  terminated?: boolean;
}

const BASH_TOOLS: ReadonlySet<string> = new Set(['run_bash', 'run_bash_background']);

/** The command head shown on the Bash narrow / session buttons — mirror of the
 *  engine's `first_command_token`: skip privilege/wrapper prefixes and
 *  `VAR=value` assignments, take the basename. Display-only (the engine derives
 *  the actually-stored pattern). `null` for an empty command. */
export function commandHead(command: string): string | null {
  const benign = new Set(['sudo', 'env', 'command', 'time', 'nice', 'builtin', 'exec']);
  const toks = command.trim().split(/\s+/).filter(Boolean);
  let i = 0;
  while (i < toks.length && (benign.has(toks[i]) || (!toks[i].startsWith('-') && toks[i].includes('=')))) {
    i++;
  }
  const head = toks[i];
  if (!head) return null;
  const base = head.split('/').filter(Boolean).pop();
  return base || null;
}

/** Body of a `CommandPermissionRequested` divider exchange. */
export function CommandPermissionBody({ event, resolved, terminated }: CommandPermissionBodyProps) {
  const { pending, decide } = usePermissionDecide(event.request_id, resolveCommandPermission);

  const effective = resolved ?? pending.value;
  const selected = effective ? resolvedChoice(effective) : null;
  const answered = selected !== null;

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
      row: 'primary',
      onClick: () => void decide(false),
    },
    {
      choice: 'allow',
      btnClass: 'action-btn action-btn-confirm',
      label: 'Allow once',
      ariaLabel: 'Allow this command once',
      row: 'primary',
      onClick: () => void decide(true),
    },
    {
      choice: 'session',
      btnClass: 'action-btn action-btn-confirm',
      label: 'Allow for this thread',
      ariaLabel: `Allow ${sessionLabelText ?? 'this command'} for the rest of this thread`,
      row: 'secondary',
      onClick: () => void decide(true, 'session'),
    },
    ...(narrow ? [{
      choice: 'narrow' as const,
      btnClass: 'action-btn action-btn-confirm',
      label: <>Always allow <code>{narrow}</code></>,
      ariaLabel: `Always allow ${narrow}`,
      row: 'secondary' as const,
      onClick: () => void decide(true, 'narrow'),
    }] : []),
    {
      choice: 'broad',
      btnClass: 'action-btn',
      label: 'Always allow',
      ariaLabel: isBash ? 'Always allow any shell command' : 'Always allow any Python',
      row: 'secondary',
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
      terminated={!!terminated}
    />
  );
}

/** The question line for a command-guard card: the command itself plus the
 *  risk summary the guard produced. */
export function renderCommandQuestion(command: string, summary: string) {
  return (
    <>
      The Lucidos Agent wants to run <code>{command}</code>. {summary} Allow?
    </>
  );
}

// ---------------------------------------------------------------------------
// MCP permission card — the chat mirror of the above for MCP server tool calls.
// Different event shape (a server + tool identity and an args summary, not a
// command or a structured tool input) and a different consent endpoint, but the
// same buttons + answered-state machinery. The "Always allow" scopes persist to
// ~/.lucidos/mcp-allowed-tools: narrow → Mcp(server:tool), broad → Mcp(server:*).
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
  resolved?: { allowed: boolean; reason?: string; persist_scope?: PersistScope };
  terminated?: boolean;
}

/** Body of an `McpPermissionRequested` divider exchange. */
export function McpPermissionBody({ event, resolved, terminated }: McpPermissionBodyProps) {
  const { pending, decide } = usePermissionDecide(event.request_id, resolveMcpPermission);

  const effective = resolved ?? pending.value;
  const selected = effective ? resolvedChoice(effective) : null;
  const answered = selected !== null;

  const buttons: ButtonSpec[] = [
    {
      choice: 'deny',
      btnClass: 'action-btn action-btn-danger',
      label: 'Deny',
      ariaLabel: `Deny calling ${event.tool_name} on ${event.server_name}`,
      row: 'primary',
      onClick: () => void decide(false),
    },
    {
      choice: 'allow',
      btnClass: 'action-btn action-btn-confirm',
      label: 'Allow once',
      ariaLabel: 'Allow this MCP tool call once',
      row: 'primary',
      onClick: () => void decide(true),
    },
    {
      choice: 'session',
      btnClass: 'action-btn action-btn-confirm',
      label: 'Allow for this thread',
      ariaLabel: `Allow ${event.tool_name} for the rest of this thread`,
      row: 'secondary',
      onClick: () => void decide(true, 'session'),
    },
    {
      choice: 'narrow',
      btnClass: 'action-btn action-btn-confirm',
      label: <>Always allow <code>{event.tool_name}</code></>,
      ariaLabel: `Always allow ${event.tool_name} on ${event.server_name}`,
      row: 'secondary',
      onClick: () => void decide(true, 'narrow'),
    },
    {
      choice: 'broad',
      btnClass: 'action-btn',
      label: <>Always allow <strong>{event.server_name}</strong></>,
      ariaLabel: `Always allow any tool on ${event.server_name}`,
      row: 'secondary',
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
      terminated={!!terminated}
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
      The Lucidos Agent wants to call <strong>{toolName}</strong> on <strong>{serverName}</strong>. Allow?
      {showArgs && <pre class="permission-mcp-args"><code>{trimmed}</code></pre>}
    </>
  );
}
