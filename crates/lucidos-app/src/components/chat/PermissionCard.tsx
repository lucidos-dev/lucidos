import type { ComponentChildren } from 'preact';
import { useSignal } from '@preact/signals';
import { showToast } from '../../store/store';
import { postMcpConsent } from '../../api/client';
import type { PersistScope } from '../../store/thread-events';
import { errorDetail } from '../../utils/errorDetail';
import { preserveAtBottom } from './scrollState';

export interface PermissionEvent {
  request_id: string;
  tool_use_id: string;
  tool_name: string;
  input: Record<string, unknown>;
  summary: string;
}

export interface PermissionBodyProps {
  event: PermissionEvent;
  resolved?: { allowed: boolean; reason?: string; persist_scope?: PersistScope };
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
 *  card hides the narrow button). Keep in sync with `claude_code.rs`. */
function narrowPattern(toolName: string, input: Record<string, unknown>): string | null {
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
 *  intercepts before CC's gate. Keep in sync with `BROAD_ALLOW_INEFFECTIVE` in
 *  `claude_code.rs`. */
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

/** Body of a `CodingAgentPermissionRequest` divider exchange — rendered inside
 *  the initiator panel which provides the chrome. `pending` is an optimistic
 *  override; SSE swaps in `resolved` once the paired
 *  `CodingAgentPermissionResolved` event arrives. */
export function PermissionBody({ event, resolved }: PermissionBodyProps) {
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

  const narrow = narrowPattern(event.tool_name, event.input);
  const showBroad = !BROAD_ALLOW_INEFFECTIVE.has(event.tool_name);
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
      onClick: () => decide(false),
    },
    {
      choice: 'allow',
      btnClass: 'action-btn action-btn-confirm',
      label: 'Allow once',
      ariaLabel: 'Allow this permission request once',
      row: 'primary',
      onClick: () => decide(true),
    },
    {
      choice: 'session',
      btnClass: 'action-btn action-btn-confirm',
      label: 'Allow for this thread',
      ariaLabel: `Allow ${session ?? event.tool_name} for the rest of this thread`,
      row: 'secondary',
      onClick: () => decide(true, 'session'),
    },
    ...(narrow ? [{
      choice: 'narrow' as const,
      btnClass: 'action-btn action-btn-confirm',
      label: <>Always allow <code>{narrow}</code></>,
      ariaLabel: `Always allow ${narrow}`,
      row: 'secondary' as const,
      onClick: () => decide(true, 'narrow'),
    }] : []),
    ...(showBroad ? [{
      choice: 'broad' as const,
      btnClass: 'action-btn',
      label: 'Always allow',
      ariaLabel: `Always allow ${event.tool_name}`,
      row: 'secondary' as const,
      onClick: () => decide(true, 'broad'),
    }] : []),
  ];

  const renderButton = (spec: ButtonSpec) => {
    const isPicked = selected === spec.choice;
    const stateClass = !answered ? ''
      : isPicked ? ' cc-permission-btn-picked'
      : ' cc-permission-btn-rejected';
    return (
      <button
        type="button"
        class={`${spec.btnClass}${stateClass}`}
        onClick={answered ? undefined : spec.onClick}
        disabled={answered}
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

  return (
    <div
      class={`cc-permission-body${answered ? ' cc-permission-body-answered' : ''}`}
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
