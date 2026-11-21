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
import type { AnswerKind, QuestionOption, TypedAnswer } from '../../store/thread-events';
import { QUESTION_SUPERSEDED_NOTE, QuestionText, TypedAnswerBlock, UnsentNotice, pickAnswer, retryPick, typedAnswerBlock, useAnswerLifecycle } from './QuestionCard';
import { parseMcpToolName } from '../../utils/mcpToolName';

interface PermissionEvent {
  request_id: string;
  tool_use_id: string;
  tool_name: string;
  input: Record<string, unknown>;
  summary: string;
  /** The grant each "Always allow"-style button stores. The card renders
   *  exactly these buttons. Absent on a card from before the engine stamped
   *  it, which then offers none. */
  grant_offer?: GrantOffer;
}

interface PermissionBodyProps {
  event: PermissionEvent;
  resolved?: { allowed: boolean; reason?: string; persist_scope?: AllowScope; standing_deny?: string };
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
 *  `store/thread-events/tool-descriptions.ts`).
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
  const mcp = parseMcpToolName(toolName);
  if (mcp) return <>use the <strong>{mcp.tool}</strong> tool from <strong>{mcp.server}</strong>{on}</>;
  return <>use the <strong>{toolName}</strong> tool{on}</>;
}

/** A tool-server grant line, `Mcp(server:tool)`. */
const MCP_GRANT = /^Mcp\((.+?):(.+)\)$/;

/** An "Always allow" grant in words: `Bash(git:*)` reads "git commands". The
 *  pattern is the allowlist's syntax, which the reader never types. */
export function grantLabel(pattern: string): ComponentChildren {
  const mcp = MCP_GRANT.exec(pattern);
  if (mcp) return <>Always allow <code>{mcp[2]}</code></>;
  const m = /^(\w+)\((.+?):\*\)$/.exec(pattern);
  if (m?.[1] === 'Bash') return <>Always allow <code>{m[2]}</code> commands</>;
  if (m?.[1] === 'Skill') return <>Always allow skills from <code>{m[2]}</code></>;
  return <>Always allow <code>{pattern}</code></>;
}

/** `grantLabel` as plain text, for an accessible name. */
export function grantText(pattern: string): string {
  const mcp = MCP_GRANT.exec(pattern);
  if (mcp) return `Always allow ${mcp[2]} on ${mcp[1]}`;
  const m = /^(\w+)\((.+?):\*\)$/.exec(pattern);
  if (m?.[1] === 'Bash') return `Always allow ${m[2]} commands`;
  if (m?.[1] === 'Skill') return `Always allow skills from ${m[2]}`;
  return `Always allow ${pattern}`;
}

/** A coding-agent tool named for a sentence: a tool-server tool by its own
 *  name and its server's, never the joined `mcp__server__tool` id. */
function toolText(toolName: string): string {
  const mcp = parseMcpToolName(toolName);
  return mcp ? `The ${mcp.tool} tool from ${mcp.server}` : `The ${toolName} tool`;
}

/** Claude Code's `Bash` and Codex's `command_execution`: the two command tools
 *  a permission card is raised for. */
function isCommandTool(toolName: string): boolean {
  return toolName === 'Bash' || toolName === 'command_execution';
}

/** What an "Allow for this thread" grant covers, in words, read off the
 *  pattern the engine stamped: `Bash(git:*)` reads "git …", `Skill(x:*)` reads
 *  "x:*", `Edit(/a/b.md)` reads "b.md", and a bare tool name reads as itself. */
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

/** A *standing deny* in words: `Bash(git:*)` reads "Always deny git commands". */
export function denyText(pattern: string): string {
  const m = /^Bash\((.+?):\*\)$/.exec(pattern);
  if (m) return `Always deny ${m[1]} commands`;
  if (pattern === 'Python') return 'Always deny Python';
  if (pattern === 'Bash') return 'Always deny any shell command';
  return `Always deny ${pattern}`;
}

/** `denyText` with the program set as code, for a button face. */
export function denyLabel(pattern: string): ComponentChildren {
  const m = /^Bash\((.+?):\*\)$/.exec(pattern);
  return m ? <>Always deny <code>{m[1]}</code> commands</> : denyText(pattern);
}

/** Which button a resolved card marks as picked, or `null` when nobody pressed
 *  one. A card the engine settled was not pressed: it expired, a restart
 *  closed it, or Lucidos allowed it on its own. So it marks none, and the note
 *  under the card says what happened. A verdict with no engine reason is a
 *  click, including an old refusal from before reasons, which reads as Deny. */
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

/** The reason a user's own Deny click carries. Mirrors `DENIAL_REASON` in the
 *  engine's `cc_permission.rs`, `command_permission.rs` and `mcp_permission.rs`. */
const USER_DENIAL_REASON = 'User denied';

/** The engine's resolution reasons, by their opening words, in plain words.
 *  The engine's own text is written for the agent, which reads it too, so the
 *  card translates rather than the engine rewording. Mirrors the `*_REASON`
 *  constants in `cc_permission.rs`, `command_permission.rs` and
 *  `mcp_permission.rs`. Every lane's restart sweep writes
 *  `RESTART_CLOSED_REASON`. */
export const PLAIN_RESOLUTION_NOTES: [string, string][] = [
  ['Interrupted by an engine restart', 'Closed when Lucidos restarted. Nobody denied it.'],
  ['Superseded by a new message', 'Closed because you sent a new message instead.'],
  ['Canceled by user', 'Closed because you stopped the reply.'],
  ['Coding agent session ended before answering', 'Closed because the agent finished its turn before anyone answered.'],
  ['Auto-allowed: file write inside', 'Allowed by Lucidos: the agent was writing a file in its own working copy.'],
  ['Auto-allowed: the command guard', 'Allowed by Lucidos: its command check judged this safe to run.'],
];

/** Reasons the engine no longer writes. Older cards in a thread's history
 *  still carry them, so the card keeps translating them. The restart sweeps
 *  wrote the first two before they shared the restart reason. The rest come
 *  from the retired unattended lane (ADR 0641). */
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

/** What a card the ENGINE resolved says under its buttons, in plain words.
 *  `null` for a user click, whose picked button says it all. A reason with no
 *  translation shows its first sentence, so a new one is never silent. */
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
  choice: PermissionCardChoice;
  btnClass: string;
  label: ComponentChildren;
  ariaLabel: string;
  onClick: () => void;
  /** A lasting choice. It sits in the Remember menu, not in the row. */
  remembered?: boolean;
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
export const DEFAULT_PERMISSION_CHOICE: PermissionCardChoice = 'allow';

/** Render one permission button with answered/terminated state styling. Shared
 *  by all three cards: coding-agent, command-guard and MCP. */
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
      {isPicked && state.mark === 'spinner' && <span class="mini-spinner permission-btn-sending" aria-hidden="true" />}
      {isPicked && state.mark === 'mark' && <span class="permission-btn-check" aria-hidden="true">✓ </span>}
      {spec.label}
    </button>
  );
}

type ButtonState = { selected: PermissionCardChoice | null; answered: boolean; mark: PickMark; terminated: boolean };

/** Remember holds the lasting choices. A resolved card shows the remembered
 * pick or a disabled Remember button for Deny and Allow once. */
function RememberSlot({ specs, live, state }: { specs: ButtonSpec[]; live: boolean; state: ButtonState }) {
  if (specs.length === 0) return null;
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

/** Auto-answer is on, but no judge model resolves, so the engine carded this
 *  command. Says so, in the engine's words, and links to the picker. Draws
 *  nothing while the judge has a model. */
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

/** The shared card chrome: a question and one wrapping row of buttons. Each
 *  permission card (coding-agent, command-guard, MCP, engine) builds its own
 *  `buttons` and feeds them here. */
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
  /** The agent question this card answers, when the engine raised it as one.
   *  A typed reply then lands on the card (`scrollState.ts`). */
  toolUseId?: string;
  question: ComponentChildren;
  buttons: ButtonSpec[];
  selected: PermissionCardChoice | null;
  answered: boolean;
  mark: PickMark;
  terminated: boolean;
  note: string | null;
  /** What sits under the buttons and the note, such as a pick that was not sent. */
  footer?: ComponentChildren;
  /** Where a pending command card points: the auto-answer switch while it
   *  is off, else the judge model, shown only when no judge model resolves. */
  settingsLink?: 'auto-answer' | 'judge-model' | null;
}) {
  const bodyStateClass = answered ? ' permission-body-answered'
    : terminated ? ' permission-body-terminated'
    : '';
  const state = { selected, answered, mark, terminated };
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
        <RememberSlot specs={buttons.filter(spec => spec.remembered)} live={live} state={state} />
        {buttons.filter(spec => !spec.remembered).map(spec => renderPermissionButton(spec, state))}
      </div>
      {note && <div class="permission-resolution-note">{note}</div>}
      {footer}
    </div>
  );
}

type PendingVerdict = { allowed: boolean; persist_scope?: AllowScope; always_deny?: boolean };

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
  resolve: (id: string, allowed: boolean, persist?: AllowScope, alwaysDeny?: boolean) => Promise<void>,
  broadGrant: string,
) {
  // Once the resolution lands, drain the pick and run the landing it deferred.
  useEffect(() => {
    if (!resolved) return;
    const localPick = pendingVerdicts.map.value.get(requestId);
    if (localPick?.persist_scope === 'broad' && resolved.allowed && resolved.persist_scope === 'broad') {
      showToast(`${broadGrant} is now allowed without asking, in this workspace.`, 'info');
    }
    pendingVerdicts.clear(requestId);
    followConfirmedCard(requestId);
  }, [resolved, requestId, broadGrant]);
  // A dead card draws no unconfirmed pick, matching its "Unresolved" header.
  // The pick stays stored, so a resolution that still lands shows it again.
  const pending = terminated ? undefined : pendingVerdicts.map.value.get(requestId);
  const mark = usePickMark(!!resolved, !!pending);
  const decide = async (allowed: boolean, persist?: AllowScope, alwaysDeny?: string) => {
    handAnsweredCardFocusToPrompt();
    pendingVerdicts.set(requestId, { allowed, persist_scope: persist, ...(alwaysDeny ? { always_deny: true } : {}) });
    // Deciding a card is a SUBMIT: the agent is expected to respond to it, so it
    // gets the same one reaction every other submit gets, anchored on this card's
    // own turn, once the engine confirms it. All three permission-shaped cards
    // decide through this hook, so one call site serves them all. Called before
    // the awaited POST, so the reader's scroll during the round trip can still
    // cancel it.
    followResolvedPermission(requestId);
    try {
      await resolve(requestId, allowed, persist, !!alwaysDeny);
      if (!allowed && alwaysDeny) {
        // A deny matches the program's name. It protects no file.
        showToast(`${denyText(alwaysDeny)}: set as a command rule for this workspace. It matches the command's name and protects no file. Remove it in Settings > Permissions.`, 'info');
      }
    } catch (e) {
      pendingVerdicts.clear(requestId);
      showToast(`Could not send decision: ${errorDetail(e)}`, 'error');
    }
  };
  return { effective: resolved ?? pending, mark, decide };
}

/** What an "Always allow" button for a broad grant says, and its accessible name. */
type BroadWords = { label: ComponentChildren; aria: string };

const plainWords = (text: string): BroadWords => ({ label: text, aria: text });

/** The grant buttons for exactly the scopes the offer names (ADR 0571). */
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

/** Why a card offers no grant at all, when the engine withheld every one. */
export function withheldNote(offer: GrantOffer): string | null {
  if (offer.withheld === 'standing-deny-unavailable' || offer.withheld === 'standing-deny-unavailable-secret-read') {
    const note = 'Couldn’t check Always deny rules. Review this command before allowing it once.';
    return offer.withheld === 'standing-deny-unavailable-secret-read'
      ? `${note} This reads Lucidos’s private files.`
      : note;
  }
  return offer.withheld === 'secret-read'
    ? "This reads Lucidos's private files. No grant can cover that, so it can only be allowed once."
    : null;
}

/** Body of a `CodingAgentPermissionRequest` divider exchange — rendered inside
 *  the initiator panel which provides the chrome. `pending` is an optimistic
 *  override; SSE swaps in `resolved` once the paired
 *  `CodingAgentPermissionResolved` event arrives. */
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
  /** The head the engine stores for a bash grant. Absent on an older card. */
  grant_head?: string;
  /** The grant each "Always allow"-style button stores. Absent on a card from
   *  before the engine stamped it, which then offers none. */
  grant_offer?: GrantOffer;
}

interface CommandPermissionBodyProps {
  event: CommandPermissionEvent;
  resolved?: { allowed: boolean; reason?: string; persist_scope?: AllowScope; standing_deny?: string };
  terminated?: boolean;
}

const BASH_TOOLS: ReadonlySet<string> = new Set(['run_bash', 'run_bash_background']);

/** Body of a `CommandPermissionRequested` divider exchange. */
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

/** The grant lines an MCP tool call's lasting choices store: the tool alone
 *  for this thread or for good, and every tool on its server. */
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

/** Body of an `McpPermissionRequested` divider exchange. */
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

// ---------------------------------------------------------------------------
// Engine permission request card. The engine raised and worded the request, and
// each option carries what picking it does (`permission_request.choices`). The
// card sorts the options into the one permission shape: Deny, Allow once, and
// a Remember menu holding the lasting choices. An action with no lasting choice
// has no Remember menu.
// ---------------------------------------------------------------------------

/** Where an engine option sits on the card. */
export type PermissionSlot = 'deny' | 'once' | 'remember' | 'other';

/** The slot a choice takes. A grant that lasts is remembered, and a choice the
 *  engine did not describe is an ordinary button, never a hidden one. */
export function permissionSlot(choice: PermissionChoice | undefined): PermissionSlot {
  switch (choice?.kind) {
    case 'refuse': return 'deny';
    case 'refuse_standing': return 'remember';
    case 'grant': return choice.lifetime === 'once' ? 'once' : 'remember';
    default: return 'other';
  }
}

const SLOT_ORDER: Record<PermissionSlot, number> = { deny: 0, once: 1, other: 2, remember: 3 };

/** The card's buttons, from the request's own options. Deny and Allow once
 *  are each one button, so a second option claiming the slot is an ordinary
 *  button rather than a duplicate. `pick` answers the card with one option. */
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
    const ariaLabel = option.description ? `${option.label}. ${option.description}` : option.label;
    const spec: ButtonSpec & { optionId: string } = {
      optionId: option.id,
      choice: slot === 'deny' ? 'deny' : slot === 'once' ? 'allow' : `option:${option.id}`,
      btnClass: slot === 'deny' || choice?.kind === 'revoke' || choice?.kind === 'refuse_standing'
        ? 'action-btn action-btn-danger'
        : slot === 'once' ? 'action-btn action-btn-confirm'
        : 'action-btn action-btn-secondary',
      label: slot === 'deny' ? 'Deny' : slot === 'once' ? 'Allow once' : option.label,
      ariaLabel,
      onClick: () => pick(option.id),
      remembered: slot === 'remember',
    };
    return { slot, spec };
  });
  return specs.sort((a, b) => SLOT_ORDER[a.slot] - SLOT_ORDER[b.slot]).map(({ spec }) => spec);
}

/** What a closed card says when its answer picked no option. */
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

/** Body of a `UserQuestionAsked` divider that carries a permission request. */
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
  // A typed answer still sending is the card's answer, as on a question card.
  const typedSending: AnswerKind | undefined = typedAnswer?.state === 'sending'
    ? { kind: 'FreeText', text: typedAnswer.text, image_hashes: typedAnswer.image_hashes }
    : undefined;
  // A dead card draws no unconfirmed pick, matching its "Unanswered" header.
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
