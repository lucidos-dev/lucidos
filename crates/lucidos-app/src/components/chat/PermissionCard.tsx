import type { ComponentChildren } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
import { showToast } from '../../store/store';
import { openCodingAgentAutoAnswerSettings, openCommandJudgeModelSettings } from '../../store/actions/menu';
import { useBackgroundModels } from '../settings/useBackgroundModels';
import { PREF_MODEL_COMMAND_JUDGE } from '@lucidos/preference-catalog';
import { currentCodingAgentAutoAnswer } from '../../store/actions/preferences';
import { ChevronDownIcon, SettingsIcon } from '../shared/icons';
import { OverflowMenu } from '../shared/OverflowMenu';
import { resolveCodingAgentPermission, resolveCommandPermission, resolveMcpPermission } from '../../store/actions/permissions';
import { changeKindName, type AllowScope } from '../../store/thread-events';
import { errorDetail } from '../../utils/errorDetail';
import { CHOICE_CARD_ROLE, handAnsweredCardFocusToPrompt, handleChoiceCardKeyDown, seedChoiceCardFocus } from './choiceCardNav';
import { pendingAnswers, pendingDecisions } from '../../store/pendingDecisions';
import { usePickMark, type PickMark } from '../../hooks/usePickMark';
import { followConfirmedCard, followResolvedPermission } from './scrollState';
import type { GrantOffer, PermissionChoice, PermissionRequest } from '../../generated/thread-event-wire';
import { UNPROVEN_WORKSPACE_DESTRUCTION_NOTE } from '../../generated/permission-notes';
import type { AnswerKind, QuestionOption, TypedAnswer } from '../../store/thread-events';
import { QUESTION_SUPERSEDED_NOTE, QuestionText, TypedAnswerBlock, UnsentNotice, pickAnswer, retryPick, typedAnswerBlock, useAnswerLifecycle } from './QuestionCard';
import { parseMcpToolName } from '../../utils/mcpToolName';

interface PermissionEvent {
  request_id: string;
  tool_use_id: string;
  tool_name: string;
  input: Record<string, unknown>;
  summary: string;
  grant_offer?: GrantOffer;
}

interface PermissionBodyProps {
  event: PermissionEvent;
  resolved?: { allowed: boolean; reason?: string; persist_scope?: AllowScope; standing_deny?: string };
  terminated?: boolean;
}

const CHANGE_VERBS: ReadonlyMap<string, string> = new Map([
  ['add', 'create'],
  ['update', 'change'],
  ['delete', 'delete'],
]);
const DEFAULT_CHANGE_VERB = 'change';

function changeVerb(kind: unknown): string {
  return CHANGE_VERBS.get(changeKindName(kind)) ?? DEFAULT_CHANGE_VERB;
}

function fileChanges(input: Record<string, unknown>): { verb: string; path: string }[] {
  const raw = input.changes;
  if (!Array.isArray(raw) || raw.length === 0) return [];
  const changes: { verb: string; path: string }[] = [];
  for (const entry of raw) {
    const path = entry && typeof entry === 'object' ? (entry as { path?: unknown }).path : null;
    if (typeof path !== 'string' || !path) return [];
    changes.push({ verb: changeVerb((entry as { kind?: unknown }).kind), path });
  }
  return changes;
}

export const REASON_HEADING = "The agent's reason";

function CardShape({ question, facts = null, payload = '', reason = '' }: {
  question: ComponentChildren;
  facts?: ComponentChildren;
  payload?: string;
  reason?: string;
}) {
  return (
    <div class="question-text markdown-content">
      <p><strong>{question}</strong></p>
      {facts}
      {payload && <pre><code>{payload}</code></pre>}
      {reason && (
        <blockquote>
          <p><strong>{REASON_HEADING}</strong></p>
          <p>{reason}</p>
        </blockquote>
      )}
    </div>
  );
}

function renderFileChangeQuestion(input: Record<string, unknown>) {
  const reason = inputText(input, 'reason');
  const changes = fileChanges(input);
  if (changes.length === 0) {
    return <CardShape question="Let the coding agent change files?" reason={reason} />;
  }
  if (changes.length === 1) {
    return (
      <CardShape
        question={<>Let the coding agent {changes[0].verb} <code>{changes[0].path}</code>?</>}
        reason={reason}
      />
    );
  }
  const verb = changes.every(c => c.verb === changes[0].verb) ? changes[0].verb : DEFAULT_CHANGE_VERB;
  return (
    <CardShape
      question={`Let the coding agent ${verb} these ${changes.length} files?`}
      facts={
        <ul>
          {changes.map((c, i) => <li key={i}>{c.verb} <code>{c.path}</code></li>)}
        </ul>
      }
      reason={reason}
    />
  );
}

function inputText(input: Record<string, unknown>, key: string): string {
  const value = input[key];
  return typeof value === 'string' ? value.trim() : '';
}

function renderCommandAsk(reason: string, command: string) {
  return <CardShape question="Let the coding agent run this command?" payload={command} reason={reason} />;
}

export function renderQuestion(
  toolName: string,
  summary: string,
  input: Record<string, unknown> = {},
) {
  if (toolName === 'file_change') return renderFileChangeQuestion(input);
  const command = typeof input.command === 'string' && input.command.trim() ? input.command : '';
  if (toolName === 'command_execution') {
    return command
      ? renderCommandAsk(inputText(input, 'reason'), command)
      : <CardShape question="Let the coding agent run a command?" reason={inputText(input, 'reason')} />;
  }
  if (toolName === 'Bash' && command) {
    return renderCommandAsk(inputText(input, 'description'), command);
  }
  const space = summary.indexOf(' ');
  const arg = space === -1 ? null : summary.slice(space + 1);
  return (
    <CardShape
      question={arg
        ? <>Let the coding agent {toolAction(toolName, true)} <code>{arg}</code>?</>
        : <>Let the coding agent {toolAction(toolName, false)}?</>}
    />
  );
}

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
  const mcp = parseMcpToolName(toolName);
  if (mcp) return <>use <code>{mcp.tool}</code> from {mcp.server}{on}</>;
  return <>use the <code>{toolName}</code> tool{on}</>;
}

const MCP_GRANT = /^Mcp\((.+?):(.+)\)$/;

export function grantLabel(pattern: string): ComponentChildren {
  const mcp = MCP_GRANT.exec(pattern);
  if (mcp) return <>Always allow <code>{mcp[2]}</code></>;
  const m = /^(\w+)\((.+?):\*\)$/.exec(pattern);
  if (m?.[1] === 'Bash') return <>Always allow <code>{m[2]}</code> commands</>;
  if (m?.[1] === 'Skill') return <>Always allow skills from <code>{m[2]}</code></>;
  return <>Always allow <code>{pattern}</code></>;
}

export function grantText(pattern: string): string {
  const mcp = MCP_GRANT.exec(pattern);
  if (mcp) return `Always allow ${mcp[2]} on ${mcp[1]}`;
  const m = /^(\w+)\((.+?):\*\)$/.exec(pattern);
  if (m?.[1] === 'Bash') return `Always allow ${m[2]} commands`;
  if (m?.[1] === 'Skill') return `Always allow skills from ${m[2]}`;
  return `Always allow ${pattern}`;
}

function toolText(toolName: string): string {
  const mcp = parseMcpToolName(toolName);
  return mcp ? `The ${mcp.tool} tool from ${mcp.server}` : `The ${toolName} tool`;
}

function isCommandTool(toolName: string): boolean {
  return toolName === 'Bash' || toolName === 'command_execution';
}

export function sessionLabel(pattern: string): string {
  const mcp = MCP_GRANT.exec(pattern);
  if (mcp) return mcp[2];
  const m = /^(\w+)\((.+)\)$/.exec(pattern);
  if (!m) return pattern;
  const [, tool, arg] = m;
  if (tool === 'Skill') return arg;
  if (arg.endsWith(':*')) return `${arg.slice(0, -2)} …`;
  return arg.split('/').filter(Boolean).pop() || arg;
}

export type PermissionCardChoice = 'deny' | 'always-deny' | 'allow' | 'session' | 'narrow' | 'broad' | `option:${string}`;

export function denyText(pattern: string): string {
  const m = /^Bash\((.+?):\*\)$/.exec(pattern);
  if (m) return `Always deny ${m[1]} commands`;
  if (pattern === 'Python') return 'Always deny Python';
  if (pattern === 'Bash') return 'Always deny any shell command';
  return `Always deny ${pattern}`;
}

export function denyLabel(pattern: string): ComponentChildren {
  const m = /^Bash\((.+?):\*\)$/.exec(pattern);
  return m ? <>Always deny <code>{m[1]}</code> commands</> : denyText(pattern);
}

export function resolvedChoice(resolved: {
  allowed: boolean;
  persist_scope?: AllowScope;
  reason?: string;
  standing_deny?: string;
  always_deny?: boolean;
}): PermissionCardChoice | null {
  if (engineResolutionNote(resolved)) return null;
  if (!resolved.allowed) {
    const standing = resolved.standing_deny || resolved.always_deny;
    return standing ? 'always-deny' : 'deny';
  }
  return resolved.persist_scope ?? 'allow';
}

const USER_DENIAL_REASON = 'User denied';

export const PLAIN_RESOLUTION_NOTES: [string, string][] = [
  ['Interrupted by an engine restart', 'Closed when Lucidos restarted. Nobody denied it.'],
  ['Superseded by a new message', 'Closed because you sent a new message instead.'],
  ['Canceled by user', 'Closed because you stopped the reply.'],
  ['Coding agent session ended before answering', 'Closed because the agent finished its turn before anyone answered.'],
  ['Auto-allowed: file write inside', 'Allowed by Lucidos: the agent was writing a file in its own working copy.'],
  ['Auto-allowed: the command guard', 'Allowed by Lucidos: its command check judged this safe to run.'],
];

export const LEGACY_RESOLUTION_NOTES: [string, string][] = [
  ['Coding agent terminated before answering', 'Closed when Lucidos restarted. Nobody denied it.'],
  ['Chat turn ended before answering', 'Closed when Lucidos restarted. Nobody denied it.'],
  ['Auto-allowed: benign in-workspace operation', 'Allowed by Lucidos: a harmless step in a trigger run with nobody to ask.'],
  ['Auto-allowed: covered by the trigger', 'Allowed by Lucidos: the trigger was given permission for this kind of action.'],
  ['Auto-denied: catastrophic operation', 'Refused by Lucidos: this is never allowed in a run with nobody to ask.'],
  ['Auto-denied: this coding-agent session runs unattended, and the command guard', 'Refused by Lucidos: its command check could not confirm this was safe, and nobody was there to ask.'],
  ['Auto-denied: this unattended coding-agent session has no side-effect grant', 'Refused by Lucidos: this unattended run has no permission for this kind of action.'],
  ['Auto-denied: nobody answered this permission card', 'Refused by Lucidos: nobody answered this card in time.'],
];

export function engineResolutionNote(
  resolved: { allowed: boolean; reason?: string } | null | undefined,
): string | null {
  const reason = resolved?.reason?.trim();
  if (!reason || reason === USER_DENIAL_REASON) return null;
  const plain = [...PLAIN_RESOLUTION_NOTES, ...LEGACY_RESOLUTION_NOTES]
    .find(([opening]) => reason.startsWith(opening));
  if (plain) return plain[1];
  const end = reason.search(/\.\s/);
  return end === -1 ? reason : reason.slice(0, end + 1);
}

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
  choice: PermissionCardChoice;
  btnClass: string;
  label: ComponentChildren;
  ariaLabel: string;
  onClick: () => void;
  remembered?: boolean;
};

export const DEFAULT_PERMISSION_CHOICE: PermissionCardChoice = 'allow';

function renderPermissionButton(
  spec: ButtonSpec,
  state: ButtonState,
) {
  const isPicked = state.selected === spec.choice;
  const { disabled, stateClass } = permissionButtonState({
    answered: state.answered,
    terminated: state.terminated,
    isPicked,
  });
  return (
    <button
      key={spec.choice}
      type="button"
      class={`${spec.btnClass}${stateClass}`}
      onClick={disabled ? undefined : spec.onClick}
      disabled={disabled}
      aria-pressed={state.answered ? isPicked : undefined}
      aria-label={spec.ariaLabel}
      data-default-choice={spec.choice === DEFAULT_PERMISSION_CHOICE && !disabled ? 'true' : undefined}
    >
      {isPicked && state.mark === 'spinner' && <span class="mini-spinner permission-btn-sending" aria-hidden="true" />}
      {isPicked && state.mark === 'mark' && <span class="permission-btn-check" aria-hidden="true">✓ </span>}
      {spec.label}
    </button>
  );
}

type ButtonState = { selected: PermissionCardChoice | null; answered: boolean; mark: PickMark; terminated: boolean };

function RememberSlot({ specs, besideOnce, live, state }: {
  specs: ButtonSpec[]; besideOnce: boolean; live: boolean; state: ButtonState;
}) {
  if (specs.length === 0) return null;
  if (specs.length === 1 || !besideOnce) return <>{specs.map(spec => renderPermissionButton(spec, state))}</>;
  if (!live) {
    const picked = specs.find(spec => spec.choice === state.selected);
    if (picked) return renderPermissionButton(picked, state);
    return (
      <button
        type="button"
        class={`action-btn action-btn-secondary${state.answered ? ' permission-btn-rejected' : ''}`}
        disabled
      >
        Remember
      </button>
    );
  }
  return (
    <OverflowMenu
      ariaLabel="Remember this answer"
      protectedSurface
      face={{
        class: 'action-btn action-btn-secondary permission-remember',
        children: <>Remember<ChevronDownIcon size="0.875rem" /></>,
      }}
      items={({ run }) => specs.map(spec => (
        <button
          key={spec.choice}
          type="button"
          role="menuitem"
          class={spec.choice === 'always-deny' ? 'thread-overflow-item thread-overflow-item-danger' : 'thread-overflow-item'}
          aria-label={spec.ariaLabel}
          onClick={run(spec.onClick)}
        >
          {spec.label}
        </button>
      ))}
    />
  );
}

function JudgeModelLink() {
  const background = useBackgroundModels();
  const why = background.row(PREF_MODEL_COMMAND_JUDGE.key)?.unresolved;
  if (!why) return null;
  return (
    <button
      type="button"
      class="permission-settings-link"
      data-not-choice="true"
      data-tooltip={why}
      onClick={openCommandJudgeModelSettings}
    >
      <SettingsIcon />
      Pick a judge model so Lucidos can answer
    </button>
  );
}

function PermissionBodyShell({
  requestId,
  toolUseId,
  question,
  buttons,
  selected,
  answered,
  mark,
  terminated,
  note,
  footer = null,
  settingsLink = null,
}: {
  requestId: string;
  toolUseId?: string;
  question: ComponentChildren;
  buttons: ButtonSpec[];
  selected: PermissionCardChoice | null;
  answered: boolean;
  mark: PickMark;
  terminated: boolean;
  note: string | null;
  footer?: ComponentChildren;
  settingsLink?: 'auto-answer' | 'judge-model' | null;
}) {
  const bodyStateClass = answered ? ' permission-body-answered'
    : terminated ? ' permission-body-terminated'
    : '';
  const state = { selected, answered, mark, terminated };
  const live = !answered && !terminated;
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (live) seedChoiceCardFocus(ref.current, requestId);
  }, [live, requestId]);
  return (
    <div
      class={`permission-body protected-surface${bodyStateClass}`}
      data-request-id={requestId}
      data-tool-use-id={toolUseId}
      data-role={live ? CHOICE_CARD_ROLE : undefined}
      ref={ref}
      onKeyDown={live ? (e) => handleChoiceCardKeyDown(e, ref.current) : undefined}
    >
      <div class="permission-text">{question}</div>
      <div class="permission-actions">
        {settingsLink === 'auto-answer' && live && (
          <button
            type="button"
            class="permission-settings-link"
            data-not-choice="true"
            onClick={openCodingAgentAutoAnswerSettings}
          >
            <SettingsIcon />
            Let Lucidos answer these
          </button>
        )}
        {settingsLink === 'judge-model' && live && <JudgeModelLink />}
        <RememberSlot
          specs={buttons.filter(spec => spec.remembered)}
          besideOnce={buttons.some(spec => spec.choice === 'allow')}
          live={live}
          state={state}
        />
        {buttons.filter(spec => !spec.remembered).map(spec => renderPermissionButton(spec, state))}
      </div>
      {note && <div class="permission-resolution-note">{note}</div>}
      {footer}
    </div>
  );
}

type PendingVerdict = { allowed: boolean; persist_scope?: AllowScope; always_deny?: boolean };

export const pendingVerdicts = pendingDecisions<PendingVerdict>();

function usePermissionDecide(
  requestId: string,
  resolved: PendingVerdict | undefined,
  terminated: boolean | undefined,
  resolve: (id: string, allowed: boolean, persist?: AllowScope, alwaysDeny?: boolean) => Promise<void>,
  broadGrant: string,
) {
  useEffect(() => {
    if (!resolved) return;
    const localPick = pendingVerdicts.map.value.get(requestId);
    if (localPick?.persist_scope === 'broad' && resolved.allowed && resolved.persist_scope === 'broad') {
      showToast(`${broadGrant} is now allowed without asking, in this workspace.`, 'info');
    }
    pendingVerdicts.clear(requestId);
    followConfirmedCard(requestId);
  }, [resolved, requestId, broadGrant]);
  const pending = terminated ? undefined : pendingVerdicts.map.value.get(requestId);
  const mark = usePickMark(!!resolved, !!pending);
  const decide = async (allowed: boolean, persist?: AllowScope, alwaysDeny?: string) => {
    handAnsweredCardFocusToPrompt();
    pendingVerdicts.set(requestId, { allowed, persist_scope: persist, ...(alwaysDeny ? { always_deny: true } : {}) });
    followResolvedPermission(requestId);
    try {
      await resolve(requestId, allowed, persist, !!alwaysDeny);
      if (!allowed && alwaysDeny) {
        showToast(`${denyText(alwaysDeny)}: set as a command rule for this workspace. It matches the command's name and protects no file. Remove it in Settings > Permissions.`, 'info');
      }
    } catch (e) {
      pendingVerdicts.clear(requestId);
      showToast(`Could not send decision: ${errorDetail(e)}`, 'error');
    }
  };
  return { effective: resolved ?? pending, mark, decide };
}

type BroadWords = { label: ComponentChildren; aria: string };

const plainWords = (text: string): BroadWords => ({ label: text, aria: text });

function grantButtons(
  { session, narrow, broad, deny }: GrantOffer,
  broadWords: (broad: string) => BroadWords,
  decide: (allowed: boolean, scope?: AllowScope, alwaysDeny?: string) => Promise<void>,
): ButtonSpec[] {
  return [
    ...(session ? [{
      choice: 'session' as const,
      btnClass: 'action-btn action-btn-secondary',
      label: 'Allow for this thread',
      ariaLabel: `Allow ${sessionLabel(session)} for the rest of this thread`,
      onClick: () => void decide(true, 'session'),
      remembered: true,
    }] : []),
    ...(narrow ? [{
      choice: 'narrow' as const,
      btnClass: 'action-btn action-btn-secondary',
      label: grantLabel(narrow),
      ariaLabel: grantText(narrow),
      onClick: () => void decide(true, 'narrow'),
      remembered: true,
    }] : []),
    ...(broad ? [{
      choice: 'broad' as const,
      btnClass: 'action-btn action-btn-secondary',
      label: broadWords(broad).label,
      ariaLabel: broadWords(broad).aria,
      onClick: () => void decide(true, 'broad'),
      remembered: true,
    }] : []),
    ...(deny ? [{
      choice: 'always-deny' as const,
      btnClass: 'action-btn action-btn-danger',
      label: denyLabel(deny),
      ariaLabel: denyText(deny),
      onClick: () => void decide(false, undefined, deny),
      remembered: true,
    }] : []),
  ];
}

export function withheldNote(offer: GrantOffer): string | null {
  if (offer.withheld === 'standing-deny-unavailable' || offer.withheld === 'standing-deny-unavailable-secret-read') {
    const note = 'Couldn’t check Always deny rules. Review this command before allowing it once.';
    return offer.withheld === 'standing-deny-unavailable-secret-read'
      ? `${note} This reads Lucidos’s private files.`
      : note;
  }
  if (offer.withheld === 'unproven-workspace-destruction') {
    return UNPROVEN_WORKSPACE_DESTRUCTION_NOTE;
  }
  return offer.withheld === 'secret-read'
    ? "This reads Lucidos's private files. No grant can cover that, so it can only be allowed once."
    : null;
}

export function PermissionBody({ event, resolved, terminated }: PermissionBodyProps) {
  const { effective, mark, decide } = usePermissionDecide(
    event.request_id,
    resolved,
    terminated,
    resolveCodingAgentPermission,
    toolText(event.tool_name),
  );

  const offer = event.grant_offer ?? {};
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
    ...grantButtons(offer, (broad) => plainWords(`Always allow ${broad}`), decide),
  ];

  return (
    <PermissionBodyShell
      requestId={event.request_id}
      question={renderQuestion(event.tool_name, event.summary, event.input)}
      buttons={buttons}
      selected={selected}
      answered={answered}
      mark={mark}
      terminated={!!terminated}
      note={engineResolutionNote(resolved) ?? withheldNote(offer)}
      settingsLink={
        !isCommandTool(event.tool_name) ? null
          : currentCodingAgentAutoAnswer() ? 'judge-model'
          : 'auto-answer'
      }
    />
  );
}


interface CommandPermissionEvent {
  request_id: string;
  tool_use_id: string;
  tool_name: string;
  command: string;
  summary: string;
  grant_head?: string;
  grant_offer?: GrantOffer;
}

interface CommandPermissionBodyProps {
  event: CommandPermissionEvent;
  resolved?: { allowed: boolean; reason?: string; persist_scope?: AllowScope; standing_deny?: string };
  terminated?: boolean;
}

const BASH_TOOLS: ReadonlySet<string> = new Set(['run_bash', 'run_bash_background']);

export function CommandPermissionBody({ event, resolved, terminated }: CommandPermissionBodyProps) {
  const { effective, mark, decide } = usePermissionDecide(
    event.request_id,
    resolved,
    terminated,
    resolveCommandPermission,
    BASH_TOOLS.has(event.tool_name) ? 'Any shell command' : 'Any Python',
  );

  const selected = effective ? resolvedChoice(effective) : null;
  const answered = effective !== undefined;

  const offer = event.grant_offer ?? {};
  const broadWords = () => plainWords(BASH_TOOLS.has(event.tool_name)
    ? 'Always allow any shell command'
    : 'Always allow any Python');

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
    ...grantButtons(offer, broadWords, decide),
  ];

  return (
    <PermissionBodyShell
      requestId={event.request_id}
      question={renderCommandQuestion(event.command, event.summary)}
      buttons={buttons}
      selected={selected}
      answered={answered}
      mark={mark}
      terminated={!!terminated}
      note={engineResolutionNote(resolved) ?? withheldNote(offer)
        ?? (event.grant_offer ? null : 'This card comes from an older Lucidos, so it can only allow once.')}
    />
  );
}

export function renderCommandQuestion(command: string, summary: string) {
  return (
    <CardShape
      question="Let the Lucidos Agent run this command?"
      facts={summary.trim() && <p>{summary.trim()}</p>}
      payload={command}
    />
  );
}


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

function mcpGrantOffer({ server_name, tool_name }: McpPermissionEvent): GrantOffer {
  return {
    session: `Mcp(${server_name}:${tool_name})`,
    narrow: `Mcp(${server_name}:${tool_name})`,
    broad: `Mcp(${server_name}:*)`,
  };
}

const mcpBroadWords = (serverName: string) => (): BroadWords => ({
  label: <>Always allow <strong>{serverName}</strong></>,
  aria: `Always allow any tool on ${serverName}`,
});

export function McpPermissionBody({ event, resolved, terminated }: McpPermissionBodyProps) {
  const { effective, mark, decide } = usePermissionDecide(
    event.request_id,
    resolved,
    terminated,
    resolveMcpPermission,
    `Any tool on ${event.server_name}`,
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
    ...grantButtons(mcpGrantOffer(event), mcpBroadWords(event.server_name), decide),
  ];

  return (
    <PermissionBodyShell
      requestId={event.request_id}
      question={renderMcpQuestion(event.server_name, event.tool_name, event.arguments_summary)}
      buttons={buttons}
      selected={selected}
      answered={answered}
      mark={mark}
      terminated={!!terminated}
      note={engineResolutionNote(resolved)}
    />
  );
}

export function renderMcpQuestion(serverName: string, toolName: string, argsSummary: string) {
  const trimmed = argsSummary.trim();
  return (
    <CardShape
      question={<>Let the Lucidos Agent use <code>{toolName}</code> from {serverName}?</>}
      payload={trimmed === '{}' ? '' : trimmed}
    />
  );
}


export type PermissionSlot = 'deny' | 'once' | 'remember' | 'other';

export const CANCEL_LABEL = 'Cancel';

export function permissionSlot(choice: PermissionChoice | undefined): PermissionSlot {
  switch (choice?.kind) {
    case 'refuse': return 'deny';
    case 'refuse_standing': return 'remember';
    case 'grant': return choice.lifetime === 'once' ? 'once' : 'remember';
    default: return 'other';
  }
}

const SLOT_ORDER: Record<PermissionSlot, number> = { deny: 0, once: 1, other: 2, remember: 3 };

export function enginePermissionButtons(
  options: QuestionOption[],
  request: PermissionRequest,
  pick: (optionId: string) => void,
): (ButtonSpec & { optionId: string })[] {
  const taken = new Set<PermissionSlot>();
  const specs = options.map((option): { slot: PermissionSlot; spec: ButtonSpec & { optionId: string } } => {
    const choice = request.choices[option.id];
    let slot = permissionSlot(choice);
    if ((slot === 'deny' || slot === 'once') && taken.has(slot)) slot = 'other';
    taken.add(slot);
    const cancels = slot === 'deny' && option.label === CANCEL_LABEL;
    const ariaLabel = option.description ? `${option.label}. ${option.description}` : option.label;
    const spec: ButtonSpec & { optionId: string } = {
      optionId: option.id,
      choice: slot === 'deny' ? 'deny' : slot === 'once' ? 'allow' : `option:${option.id}`,
      btnClass: cancels ? 'action-btn action-btn-secondary'
        : slot === 'deny' || choice?.kind === 'revoke' || choice?.kind === 'refuse_standing'
        ? 'action-btn action-btn-danger'
        : slot === 'once' ? 'action-btn action-btn-confirm'
        : 'action-btn action-btn-secondary',
      label: cancels ? CANCEL_LABEL : slot === 'deny' ? 'Deny' : slot === 'once' ? 'Allow once' : option.label,
      ariaLabel,
      onClick: () => pick(option.id),
      remembered: slot === 'remember',
    };
    return { slot, spec };
  });
  return specs.sort((a, b) => SLOT_ORDER[a.slot] - SLOT_ORDER[b.slot]).map(({ spec }) => spec);
}

export function engineAnswerNote(answer: AnswerKind | undefined): string | null {
  if (answer?.kind === 'Canceled') return 'Canceled';
  if (answer?.kind === 'Superseded') return QUESTION_SUPERSEDED_NOTE;
  return null;
}

interface EnginePermissionBodyProps {
  threadId: string;
  eventId?: string;
  toolUseId: string;
  question: string;
  options: QuestionOption[];
  request: PermissionRequest;
  resolved?: AnswerKind;
  typedAnswer?: TypedAnswer;
  terminated?: boolean;
}

export function EnginePermissionBody({
  threadId,
  eventId,
  toolUseId,
  question,
  options,
  request,
  resolved,
  typedAnswer,
  terminated,
}: EnginePermissionBodyProps) {
  const unsentPick = useAnswerLifecycle(toolUseId, resolved, terminated);
  const typedSending: AnswerKind | undefined = typedAnswer?.state === 'sending'
    ? { kind: 'FreeText', text: typedAnswer.text, image_hashes: typedAnswer.image_hashes }
    : undefined;
  const pending = terminated ? undefined : pendingAnswers.map.value.get(toolUseId) ?? typedSending;
  const effective = resolved ?? pending;
  const mark = usePickMark(!!resolved, !!pending);
  const buttons = enginePermissionButtons(
    options,
    request,
    optionId => void pickAnswer(threadId, toolUseId, { kind: 'Selected', option_id: optionId }, followResolvedPermission),
  );
  const pickedId = effective?.kind === 'Selected' ? effective.option_id : null;
  const picked = buttons.find(spec => spec.optionId === pickedId);
  const unsentSelected = unsentPick?.answer.kind === 'Selected';
  return (
    <PermissionBodyShell
      requestId={toolUseId}
      toolUseId={toolUseId}
      question={<QuestionText question={question} threadId={threadId} eventId={eventId} />}
      buttons={buttons}
      selected={picked?.choice ?? null}
      answered={effective !== undefined}
      mark={mark}
      terminated={!!terminated}
      note={engineAnswerNote(effective)}
      footer={
        <>
          {unsentSelected && <UnsentNotice failure={unsentPick.failure} refusal={unsentPick.refusal} onRetry={() => void retryPick(threadId, toolUseId)} />}
          {effective?.kind === 'FreeText' && (
            <TypedAnswerBlock text={effective.text} imageHashes={effective.image_hashes ?? []} sending={mark === 'spinner'} />
          )}
          {typedAnswer && !typedSending && typedAnswerBlock(typedAnswer)}
        </>
      }
    />
  );
}
