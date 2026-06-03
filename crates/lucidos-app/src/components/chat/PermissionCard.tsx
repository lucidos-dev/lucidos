import type { ComponentChildren } from 'preact';
import { useSignal } from '@preact/signals';
import { showToast } from '../../store/store';
import { postMcpConsent } from '../../api/client';
import type { PersistScope } from '../../store/thread-events';
import { errorDetail } from '../../utils/errorDetail';
import { preserveAtBottom } from './scrollState';

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
 *  were about to grant. */
export function renderQuestion(toolName: string, summary: string) {
  const space = summary.indexOf(' ');
  const arg = space === -1 ? null : summary.slice(space + 1);
  return arg ? (
    <>
      Claude Code wants to use the <strong>{toolName}</strong> tool on <code>{arg}</code>. Allow?
    </>
  ) : (
    <>
      Claude Code wants to use the <strong>{toolName}</strong> tool. Allow?
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
 *  intercepts before CC's gate. Restricted to Bash because that's the only
 *  tool we've empirically observed surfacing the card on these paths under
 *  the user's bare-allowlist setup (Read / Edit / cat all auto-approved
 *  silently). Mirror of `input_touches_protected_path` in `claude_code.rs`. */
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
  if (toolName === 'Bash') {
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

/** Tools whose bare entry in `--allowedTools` cannot be respected by CC.
 *  Two reasons a tool ends up here:
 *    - Edit/Write/NotebookEdit: `--permission-mode acceptEdits` routes them
 *      through `--permission-prompt-tool` for CC's protected paths (`.claude/`,
 *      `.git/`) and auto-approves them everywhere else, so a bare `Edit` line
 *      in `cc-allowed-tools` never helps.
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
    : isPicked ? ' cc-permission-btn-picked'
    : ' cc-permission-btn-rejected';
  return { disabled, stateClass };
}

/** Body of a `CodingAgentPermissionRequest` divider exchange — rendered inside
 *  the initiator panel which provides the chrome. `pending` is an optimistic
 *  override; SSE swaps in `resolved` once the paired
 *  `CodingAgentPermissionResolved` event arrives. */
export function PermissionBody({ event, resolved, terminated }: PermissionBodyProps) {
  const pending = useSignal<{ allowed: boolean; persist_scope?: PersistScope } | null>(null);

  const effective = resolved ?? pending.value;

  const decide = async (allowed: boolean, persist?: PersistScope) => {
    preserveAtBottom();
    pending.value = { allowed, persist_scope: persist };
    if (allowed && persist === 'broad') {
      // Coarse trust granted — let the user feel the weight of it.
      showToast('You only live once', 'info');
    }
    try {
      await postMcpConsent(event.request_id, allowed, persist);
    } catch (e) {
      pending.value = null;
      showToast(`Could not send decision: ${errorDetail(e)}`, 'error');
    }
  };

  const touchesProtected = inputTouchesProtectedPath(event.tool_name, event.input);
  const narrow = touchesProtected ? null : narrowPattern(event.tool_name, event.input);
  const showBroad = !BROAD_ALLOW_INEFFECTIVE.has(event.tool_name) && !touchesProtected;
  const session = sessionLabel(event.tool_name, event.input);

  const selected = effective ? resolvedChoice(effective) : null;
  const answered = selected !== null;

  type ButtonSpec = {
    choice: PermissionChoice;
    btnClass: string;
    label: ComponentChildren;
    ariaLabel: string;
    row: 'primary' | 'secondary';
    onClick: () => void;
  };

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
    {
      choice: 'session',
      btnClass: 'action-btn action-btn-confirm',
      label: 'Allow for this thread',
      ariaLabel: `Allow ${session ?? event.tool_name} for the rest of this thread`,
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
    ...(showBroad ? [{
      choice: 'broad' as const,
      btnClass: 'action-btn',
      label: 'Always allow',
      ariaLabel: `Always allow ${event.tool_name}`,
      row: 'secondary' as const,
      onClick: () => void decide(true, 'broad'),
    }] : []),
  ];

  const renderButton = (spec: ButtonSpec) => {
    const isPicked = selected === spec.choice;
    const { disabled, stateClass } = permissionButtonState({
      answered,
      terminated: !!terminated,
      isPicked,
    });
    return (
      <button
        type="button"
        class={`${spec.btnClass}${stateClass}`}
        onClick={disabled ? undefined : spec.onClick}
        disabled={disabled}
        aria-pressed={answered ? isPicked : undefined}
        aria-label={spec.ariaLabel}
      >
        {isPicked && <span class="cc-permission-btn-check" aria-hidden="true">✓ </span>}
        {spec.label}
      </button>
    );
  };

  const primary = buttons.filter(b => b.row === 'primary');
  const secondary = buttons.filter(b => b.row === 'secondary');

  const bodyStateClass = answered ? ' cc-permission-body-answered'
    : terminated ? ' cc-permission-body-terminated'
    : '';
  return (
    <div
      class={`cc-permission-body${bodyStateClass}`}
      data-request-id={event.request_id}
    >
      <div class="cc-permission-text">{renderQuestion(event.tool_name, event.summary)}</div>
      <div class="cc-permission-actions">
        {primary.map(renderButton)}
      </div>
      {secondary.map(spec => (
        <div key={spec.choice} class="cc-permission-actions cc-permission-actions-secondary">
          {renderButton(spec)}
        </div>
      ))}
    </div>
  );
}
