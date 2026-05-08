import { useSignal } from '@preact/signals';
import { showToast } from '../../store/store';
import { postMcpConsent, type PersistScope } from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';

export interface PermissionEvent {
  request_id: string;
  tool_use_id: string;
  tool_name: string;
  input: Record<string, unknown>;
  summary: string;
}

export interface PermissionBodyProps {
  event: PermissionEvent;
  resolved?: { allowed: boolean; reason?: string };
}

/** Split "skill update-config" into "skill " + <strong>update-config</strong>
 *  so the meaningful arg stands out from the tool-name prefix. Used by the
 *  answered card; the active prompt uses `renderQuestion` instead so the tool
 *  identity is unmistakable. */
function renderSummary(summary: string) {
  const space = summary.indexOf(' ');
  if (space === -1) return <strong>{summary}</strong>;
  return (
    <>
      {summary.slice(0, space)} <strong>{summary.slice(space + 1)}</strong>
    </>
  );
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

/** Body of a `CodingAgentPermissionRequest` divider exchange — rendered inside
 *  the initiator panel which provides the chrome. The `pending` signal is an
 *  optimistic override — replaced by `resolved` once the paired
 *  `CodingAgentPermissionResolved` event arrives over SSE. */
export function PermissionBody({ event, resolved }: PermissionBodyProps) {
  const pending = useSignal<boolean | null>(null);

  const effective = resolved
    ?? (pending.value !== null ? { allowed: pending.value } : undefined);

  if (effective) {
    return <AnsweredBody event={event} resolved={effective} />;
  }

  const decide = async (allowed: boolean, persist?: PersistScope) => {
    pending.value = allowed;
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

  return (
    <div class="cc-permission-body" data-request-id={event.request_id}>
      <div class="cc-permission-text">{renderQuestion(event.tool_name, event.summary)}</div>
      <div class="cc-permission-actions">
        <button
          type="button"
          class="action-btn action-btn-danger"
          onClick={() => decide(false)}
          aria-label="Deny this permission request"
        >
          Deny
        </button>
        <button
          type="button"
          class="action-btn action-btn-confirm"
          onClick={() => decide(true)}
          aria-label="Allow this permission request once"
        >
          Allow once
        </button>
      </div>
      <div class="cc-permission-actions cc-permission-actions-secondary">
        <button
          type="button"
          class="action-btn action-btn-confirm"
          onClick={() => decide(true, 'session')}
          aria-label={`Allow ${session ?? event.tool_name} for the rest of this thread`}
        >
          Allow for this thread
        </button>
      </div>
      {narrow && (
        <div class="cc-permission-actions cc-permission-actions-secondary">
          <button
            type="button"
            class="action-btn action-btn-confirm"
            onClick={() => decide(true, 'narrow')}
            aria-label={`Always allow ${narrow}`}
          >
            Always allow <code>{narrow}</code>
          </button>
        </div>
      )}
      {showBroad && (
        <div class="cc-permission-actions cc-permission-actions-secondary">
          <button
            type="button"
            class="action-btn"
            onClick={() => decide(true, 'broad')}
            aria-label={`Always allow ${event.tool_name}`}
          >
            Always allow
          </button>
        </div>
      )}
    </div>
  );
}

function AnsweredBody({
  event,
  resolved,
}: {
  event: PermissionEvent;
  resolved: { allowed: boolean; reason?: string };
}) {
  return (
    <div class="cc-permission-body cc-permission-body-answered">
      <div class="cc-permission-text">{renderSummary(event.summary)}</div>
      {resolved.allowed ? (
        <div class="cc-permission-allowed-badge">Allowed</div>
      ) : (
        <div class="cc-permission-denied-badge">
          {resolved.reason ? `Denied: ${resolved.reason}` : 'Denied'}
        </div>
      )}
    </div>
  );
}
