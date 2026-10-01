/**
 * The thread review loop: one PNG per kind of turn a thread can draw. A change
 * to turn headers, cards or rows can then be LOOKED AT before it is applied.
 *
 *   THREAD_SHOTS=1 ./scripts/e2e-browser.sh --no-webkit -f thread-shots.spec.ts
 *
 * It seeds two threads straight into the events table, a Lucidos Agent thread
 * and a Claude Code thread, each holding one turn per scene below. The app
 * then renders them through the real store and the real stylesheet, so a shot
 * is what a reader sees, not a mock.
 *
 * The PNGs land in `test-results/thread-shots/<project>/`, with a
 * `scenes.json` naming each one. Skipped unless `THREAD_SHOTS=1`: it asserts
 * nothing, like header-shots and theme-shots.
 */
import { mkdirSync, writeFileSync } from 'node:fs';
import { randomUUID } from 'node:crypto';
import type { BrowserContext } from '@playwright/test';
import { test, expect, type Page } from './fixtures';
import { assertHealthy, disarmFollowSeed, isMobileViewport, navigateToApp, renderWholeTranscript } from './helpers';
import { psql } from './db-helpers';
import { HARNESS_DEVICE_ID } from './harnessDevice';

const SHOOTING = process.env.THREAD_SHOTS === '1';
const ROOT = 'test-results/thread-shots';

type SeedEvent = [type: string, payload: Record<string, unknown>];

/** One turn to shoot: the events that make it, and what the gallery says. */
interface Scene {
  slug: string;
  title: string;
  trigger: string;
  events: SeedEvent[];
}

interface SeededThread {
  threadId: string;
  title: string;
  /** `eventIds` is every event the scene seeded, so the shot can find each turn it opened. */
  scenes: Array<Scene & { anchorId: string; eventIds: string[] }>;
}

interface GalleryEntry {
  file: string;
  title: string;
  trigger: string;
  thread: string;
  /** Set on a header crop: which header system drew it. */
  header?: string;
}

/** Every header system a turn can draw, by the class of its row. */
const HEADER_KINDS: Array<[kind: string, selector: string]> = [
  ['initiator', '.initiator-header'],
  ['response', '.response-header'],
  ['event-row', '.event-row-head'],
  ['side-question', '.side-question-head'],
];

const YOU = { kind: 'device', device_id: HARNESS_DEVICE_ID };
const ENGINE = (reason: Record<string, unknown>) => ({ kind: 'engine', reason });
const PARENT_LINK = {
  kind: 'thread_link', thread_id: randomUUID(), title: 'Plan the Q4 offsite', direction: 'parent', mode: 'agent',
};

const REPLY = [
  'Here is what I found.',
  '',
  '### Options',
  '',
  '1. **Oslo**: short travel for most of the team.',
  '2. **Bergen**: better venues, one extra flight.',
  '',
  '```ts',
  'const venue = pick(options, { budget: 40_000 });',
  '```',
].join('\n');

function human(text: string, channel: string): SeedEvent {
  return ['MessageReceived', { text, mode: 'human', origin: YOU, actor: YOU, device_id: HARNESS_DEVICE_ID, channel }];
}

/** A chat tool call and its result. `_id` pins the call's event id, which the
 *  result names so the store pairs them. */
function chatTool(name: string, description: string, result: string): SeedEvent[] {
  const callId = randomUUID();
  return [
    ['ToolCalled', { name, args: {}, description, _id: callId }],
    ['ToolResult', { name, result, success: true, tool_called_event_id: callId }],
  ];
}

/** A reply as the engine writes one: streamed text, then the closing event.
 *  The transcript draws the streamed text; the closing event only ends the turn. */
function chatReply(text: string): SeedEvent[] {
  return [['TextStreamed', { text }], ['ResponseGenerated', { text, model: 'claude-opus-5-5' }]];
}

function ccReply(text: string): SeedEvent[] {
  return [['CodingAgentTextStreamed', { text, coding_agent: 'claude-code' }], ['ResponseGenerated', { text }]];
}

function ccTool(name: string, description: string, result: string): SeedEvent[] {
  const useId = randomUUID();
  return [
    ['CodingAgentToolCalled', { name, args: {}, description, coding_agent: 'claude-code', tool_use_id: useId }],
    ['CodingAgentToolResult', { name, result, coding_agent: 'claude-code', tool_use_id: useId }],
  ];
}

function chatScenes(): Scene[] {
  const sideId = randomUUID();
  const waitId = randomUUID();
  const permissionId = randomUUID();
  return [
    {
      slug: 'user-turn-with-steps',
      title: 'Your message, then a reply with steps',
      trigger: 'MessageReceived (device) + ToolCalled/ToolResult + ResponseGenerated',
      events: [
        human('Where should we hold the Q4 offsite?', 'chat'),
        ['TodoListWritten', { items: [
          { content: 'Search venues', active_form: 'Searching venues', status: 'completed' },
          { content: 'Compare travel cost', active_form: 'Comparing travel cost', status: 'in_progress' },
          { content: 'Draft the proposal', active_form: 'Drafting the proposal', status: 'pending' },
        ] }],
        ...chatTool('web_search', 'Search the web for "offsite venues Norway"', '8 results'),
        ...chatTool('read_file', 'Read artifacts/offsite/budget.md', '42 lines'),
        ...chatReply(REPLY),
      ],
    },
    {
      slug: 'side-question',
      title: 'A side question asked mid-turn',
      trigger: 'SideQuestionAsked + SideQuestionAnswered',
      events: [
        human('Draft the proposal email.', 'chat'),
        ...chatTool('read_file', 'Read artifacts/offsite/notes.md', '18 lines'),
        ['SideQuestionAsked', { side_question_id: sideId, question: 'Is Bergen reachable by train?' }],
        ['SideQuestionAnswered', { side_question_id: sideId, answer: 'Yes. The Bergen Line takes about seven hours from Oslo.' }],
        ...chatReply('The draft is in `artifacts/offsite/proposal.md`.'),
      ],
    },
    {
      slug: 'event-wait',
      title: 'A turn that waited for an event',
      trigger: 'EventWaitStarted + EventWaitDelivered',
      events: [
        human('Tell me when the venue replies.', 'chat'),
        ['EventWaitStarted', {
          wait_id: waitId, tool_use_id: randomUUID(), on: [{ event_type: 'EmailReceived' }],
          reason: 'Waiting for the venue to reply', expires_at: new Date(Date.now() + 86_400_000).toISOString(), watermark: 0,
        }],
        ['EventWaitDelivered', { wait_id: waitId, event_id: randomUUID(), event_type: 'EmailReceived', payload: { from: 'venue' }, matched_index: 0 }],
        ...chatReply('The venue replied: both dates are free.'),
      ],
    },
    {
      slug: 'command-permission',
      title: 'A command permission card (answered), then a checkpoint',
      trigger: 'CommandPermissionRequested + CommandPermissionResolved + CommandCheckpointed',
      events: [
        ['CommandPermissionRequested', {
          request_id: permissionId, tool_use_id: randomUUID(), tool_name: 'bash',
          command: 'rm -rf artifacts/offsite/old-drafts', summary: 'Delete the old drafts folder',
        }],
        ['CommandPermissionResolved', { request_id: permissionId, allowed: true, persist_scope: 'session' }],
        ['CommandCheckpointed', {
          checkpoint_id: randomUUID(), command: 'rm -rf artifacts/offsite/old-drafts',
          summary: 'Snapshot before deleting old-drafts', restores: 12, removes: 0,
        }],
        ...chatReply('Deleted the old drafts. You can undo it from the checkpoint.'),
      ],
    },
    {
      slug: 'agent-message',
      title: 'A message from another thread (agent)',
      trigger: 'MessageReceived mode=agent, origin=thread_link',
      events: [
        ['MessageReceived', { text: 'Please add the dietary list to the proposal.', mode: 'agent', origin: PARENT_LINK, actor: PARENT_LINK, channel: 'chat' }],
        ...chatReply('Added a dietary section.'),
      ],
    },
    {
      slug: 'trigger-fired',
      title: 'A trigger fired',
      trigger: 'TriggerStarted (schedule)',
      events: [
        ['TriggerStarted', {
          trigger_id: 'daily-digest', trigger_name: 'Daily digest', prompt: 'Summarise what changed in the offsite plan today.',
          invocation: { kind: 'Schedule' }, origin: ENGINE({ kind: 'scheduler', trigger_id: 'daily-digest', trigger_name: 'Daily digest' }),
        }],
        ...chatReply('Two venues replied, and the budget is unchanged.'),
      ],
    },
    {
      slug: 'child-completed',
      title: 'A sub-thread returned',
      trigger: 'ChildThreadCompleted (success)',
      events: [
        ['ChildThreadCompleted', {
          child_thread_id: randomUUID(), child_thread_title: 'Compare train and flight prices', status: 'success',
          summary: 'Trains are 30% cheaper for 9 of 12 people.',
        }],
        ...chatReply('Trains win on cost. I updated the proposal.'),
      ],
    },
    {
      slug: 'child-stopped',
      title: 'A sub-thread was stopped',
      trigger: 'ChildThreadStopped',
      events: [['ChildThreadStopped', { child_thread_id: randomUUID(), child_thread_title: 'Book the venue' }]],
    },
    {
      slug: 'response-failed',
      title: 'A reply that failed',
      trigger: 'MessageReceived + ResponseFailed',
      events: [
        human('Book both venues for a hold.', 'chat'),
        ['ResponseFailed', { error: 'Provider returned 529: overloaded' }],
      ],
    },
    {
      slug: 'response-canceled',
      title: 'A reply you stopped',
      trigger: 'MessageReceived + ResponseCanceled (user_stop)',
      events: [
        human('Write a long history of Bergen.', 'chat'),
        ['TextStreamed', { text: 'Bergen was founded in 1070 by' }],
        ['ResponseCanceled', { text: 'Bergen was founded in 1070 by', cause: 'user_stop', actor: YOU }],
      ],
    },
    {
      slug: 'response-aborted',
      title: 'A reply the system interrupted',
      trigger: 'ResponseAborted (engine_shutdown)',
      events: [
        human('Check the train timetable.', 'chat'),
        ...chatTool('web_fetch', 'Fetch the Bergen Line timetable', 'ok'),
        ['ResponseAborted', { cause: 'engine_shutdown', actor: { kind: 'system' } }],
      ],
    },
    {
      slug: 'continuation',
      title: 'The engine resumed the turn',
      trigger: 'ContinuationStarted',
      events: [
        ['ContinuationStarted', { origin: ENGINE({ kind: 'continuation_started' }), reason: 'user_clicked_continue' }],
        ...chatReply('The 08:25 train arrives at 15:20.'),
      ],
    },
  ];
}

/** The change the Claude Code thread applies. It gets a real `changes` row,
 *  because the card fetches its description and commits by id. */
function seedAppliedChange(changeId: string, threadId: string): void {
  psql('INSERT INTO changes (id, request_id, branch_name, repo_root, description, file_count, files, requires_restart, hardened, thread_id, status, commits) ' +
    `VALUES ('${changeId}', '${randomUUID()}', 'dark-mode-toggle', '/home/user/example-repo', 'Add a dark-mode toggle to Appearance', 2, ` +
    `ARRAY['src/settings/Appearance.tsx', 'src/settings/Theme.tsx'], false, true, '${threadId}', 'applied', ARRAY['feat: add a dark-mode toggle'])`);
}

function ccScenes(changeId: string): Scene[] {
  const sessionId = randomUUID();
  const answeredId = randomUUID();
  const pendingId = randomUUID();
  const permissionId = randomUUID();
  return [
    {
      slug: 'cc-user-turn-with-steps',
      title: 'Your message, then Claude Code working',
      trigger: 'MessageReceived + CodingAgentThoughtStreamed + CodingAgentToolCalled/Result + ResponseGenerated',
      events: [
        human('Add a dark-mode toggle to settings.', 'claude_code'),
        ['SessionStarted', { session_id: randomUUID(), branch: 'dark-mode-toggle', coding_agent: 'claude-code' }],
        ['CodingAgentThoughtStreamed', { text: 'The settings page already reads the theme preference.', coding_agent: 'claude-code' }],
        ...ccTool('Read', 'Read src/settings/Theme.tsx', '120 lines'),
        ...ccTool('Edit', 'Edit src/settings/Theme.tsx', 'ok'),
        ...ccTool('Bash', 'Run npm test', '42 passed'),
        ...ccReply('Added the toggle and a test. **42 tests pass.**'),
        ['CodingAgentIdled', { has_changes: true, coding_agent: 'claude-code' }],
      ],
    },
    {
      slug: 'cc-question-answered',
      title: 'A question card (answered)',
      trigger: 'UserQuestionAsked + UserQuestionAnswered',
      events: [
        ['UserQuestionAsked', {
          tool_use_id: answeredId, cc_session_id: sessionId, question: 'Where should the toggle live?',
          options: [
            { id: 'a', label: 'Appearance section', description: 'Next to the theme picker.' },
            { id: 'b', label: 'Header menu', description: 'One tap from anywhere.' },
          ],
        }],
        ['UserQuestionAnswered', { tool_use_id: answeredId, answer: { kind: 'Selected', option_id: 'a' } }],
        ...ccTool('Edit', 'Edit src/settings/Appearance.tsx', 'ok'),
        ...ccReply('Moved it to Appearance.'),
      ],
    },
    {
      slug: 'cc-permission-denied',
      title: 'A permission card (denied)',
      trigger: 'CodingAgentPermissionRequest + CodingAgentPermissionResolved',
      events: [
        ['CodingAgentPermissionRequest', {
          request_id: permissionId, tool_use_id: randomUUID(), tool_name: 'Bash',
          input: { command: 'git push origin dark-mode-toggle' }, summary: 'Run git push origin dark-mode-toggle',
        }],
        ['CodingAgentPermissionResolved', { request_id: permissionId, allowed: false, reason: 'No pushing from a worktree.' }],
        ...ccReply('Understood, I will not push.'),
      ],
    },
    {
      slug: 'cc-missing-hardening',
      title: 'The engine asked for hardening',
      trigger: 'MissingHardeningDetected',
      events: [
        ['MissingHardeningDetected', { origin: ENGINE({ kind: 'missing_hardening' }) }],
        ...ccTool('Bash', 'Run ./scripts/harden-suites.sh', 'all green'),
        ...ccReply('Hardening passed.'),
      ],
    },
    {
      slug: 'cc-merge-conflict',
      title: 'The engine found a merge conflict',
      trigger: 'MergeConflictDetected',
      events: [
        ['MergeConflictDetected', { change_id: changeId, files: ['src/settings/Theme.tsx'], origin: ENGINE({ kind: 'merge_conflict' }) }],
        ...ccTool('Edit', 'Resolve the conflict in src/settings/Theme.tsx', 'ok'),
        ...ccReply('Resolved the conflict.'),
      ],
    },
    {
      slug: 'cc-auto-prompt',
      title: 'An engine auto-prompt',
      trigger: 'UserPromptInjected (engine)',
      events: [
        ['UserPromptInjected', { text: 'Run /harden before finishing.', mode: 'engine', origin: ENGINE({ kind: 'harden_retrigger' }) }],
        ...ccReply('Done.'),
      ],
    },
    {
      slug: 'cc-change-applied',
      title: 'A change was applied',
      trigger: 'ChangeApplied',
      events: [['ChangeApplied', { change_id: changeId, commits: ['feat: add a dark-mode toggle'], thread_title: 'Dark-mode toggle' }]],
    },
    {
      slug: 'cc-change-apply-failed',
      title: 'A change failed to apply',
      trigger: 'ChangeApplyFailed',
      events: [['ChangeApplyFailed', { change_id: randomUUID(), error: 'Merge conflict in src/settings/Theme.tsx' }]],
    },
    {
      slug: 'cc-question-pending',
      title: 'A question card waiting for you, with a held message',
      trigger: 'UserQuestionAsked (unanswered) + MessageHeld',
      events: [
        ['UserQuestionAsked', {
          tool_use_id: pendingId, cc_session_id: sessionId, question: 'Ship the toggle behind a flag?',
          options: [
            { id: 'yes', label: 'Yes, behind a flag', description: 'Safer rollout.' },
            { id: 'no', label: 'No, ship it on', description: 'Simpler code.' },
          ],
        }],
        ['MessageHeld', { text: 'Also check the contrast ratio.', mode: 'agent', origin: PARENT_LINK }],
      ],
    },
  ];
}

/** Insert one thread and its scenes, one second apart so timestamps read like
 *  a real conversation. Each scene's first event id is the `data-event-id` its
 *  `.chat-exchange` carries. */
function seedThread(opts: { title: string; source: 'chat' | 'claude_code'; scenes: Scene[] }): SeededThread {
  const threadId = randomUUID();
  const start = Date.now() - 3_600_000;
  const quote = (s: string) => s.replace(/'/g, "''");
  const rows: string[] = [];
  let tick = 0;
  const scenes = opts.scenes.map((scene) => {
    const ids = scene.events.map(([, payload]) => (payload._id as string | undefined) ?? randomUUID());
    const requestId = ids[0];
    scene.events.forEach(([type, { _id, ...payload }], i) => {
      void _id;
      const body = i === 0 ? payload : { request_event_id: requestId, ...payload };
      const at = new Date(start + (tick++) * 1000).toISOString();
      rows.push(`('${ids[i]}', '${type}', '${quote(JSON.stringify(body))}'::jsonb, '${at}', 'thread', '${threadId}', '${threadId}')`);
    });
    return { ...scene, anchorId: requestId, eventIds: ids };
  });
  const now = new Date(start + tick * 1000).toISOString();
  const isCc = opts.source === 'claude_code';
  psql([
    'INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, coding_agent_proposed, coding_agent_requires_restart, coding_agent_is_external_repo) ' +
      `VALUES ('${threadId}', '${quote(opts.title)}', '${opts.source}', '${now}', ${scenes.length}, false, true, 'idle', 'inbox', 'active', ${isCc}, 0, false, false, false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES\n${rows.join(',\n')}`,
  ].join(';\n'));
  return { threadId, title: opts.title, scenes };
}

async function openThread(page: Page, threadId: string): Promise<void> {
  // Dark mode, which is what the gallery's reader runs.
  await page.addInitScript((tid: string) => {
    localStorage.setItem('lucidos-focused-thread', tid);
    localStorage.setItem('lucidos-theme-mode', 'dark');
  }, threadId);
  await disarmFollowSeed(page);
  await navigateToApp(page);
  // The boot splash is an opaque overlay that fades once the app paints. A
  // shot taken under it is a picture of the splash.
  await expect.poll(() => page.locator('.boot-splash').count(), { timeout: 30_000 }).toBe(0);
  await expect(page.locator('.chat-exchange').first()).toBeVisible({ timeout: 30_000 });
  await renderWholeTranscript(page);
}

/** One picture of every turn a scene opened. An abort or a cancel opens a turn of
 *  its own under the reply it ended, so a scene can span several. */
async function shotTurns(page: Page, path: string, eventIds: string[]): Promise<void> {
  const box = await page.evaluate((ids: string[]) => {
    const turns = [...document.querySelectorAll<HTMLElement>('.chat-exchange')]
      .filter((el) => ids.includes(el.dataset.eventId ?? '') && el.getBoundingClientRect().height > 0);
    if (turns.length === 0) return null;
    turns[0].scrollIntoView({ block: 'start' });
    const rects = turns.map((el) => el.getBoundingClientRect());
    const top = Math.min(...rects.map((r) => r.top));
    const left = Math.min(...rects.map((r) => r.left));
    return { x: left, y: top, width: Math.max(...rects.map((r) => r.right)) - left, height: Math.max(...rects.map((r) => r.bottom)) - top };
  }, eventIds);
  if (box) await page.screenshot({ path, clip: box, animations: 'disabled', caret: 'hide' });
}

/** Shoot one thread on a page of its own, so exactly one init script picks
 *  the focused thread: Playwright leaves the order of several undefined. */
async function shootThread(context: BrowserContext, dir: string, prefix: string, seeded: SeededThread): Promise<GalleryEntry[]> {
  const page = await context.newPage();
  try {
    return await shootPage(page, dir, prefix, seeded);
  } finally {
    await page.close();
  }
}

/** The whole pane, its frame (app header, title bar, composer), every scene on
 *  its own, then the surfaces a thread opens over itself. */
async function shootPage(page: Page, dir: string, prefix: string, seeded: SeededThread): Promise<GalleryEntry[]> {
  const written: GalleryEntry[] = [];
  const mobile = isMobileViewport(page);
  const opts = { animations: 'disabled', caret: 'hide' } as const;
  const shot = async (file: string, selector: string, title: string, trigger: string) => {
    const target = page.locator(selector).filter({ visible: true }).first();
    if (await target.count() === 0) return;
    await target.scrollIntoViewIfNeeded();
    await target.screenshot({ path: `${dir}/${file}`, ...opts });
    written.push({ file, title, trigger, thread: seeded.title });
  };

  await openThread(page, seeded.threadId);
  await page.screenshot({ path: `${dir}/${prefix}-00-overview.png`, ...opts });
  written.push({ file: `${prefix}-00-overview.png`, title: 'The whole pane, as opened', trigger: 'Thread opened', thread: seeded.title });

  await shot(`${prefix}-01-app-header.png`, mobile ? '.mobile-thread-header' : '.desktop-header .pane-header-brand',
    'App header band', mobile ? 'MobileThreadHeader' : 'AppHeader');
  await shot(`${prefix}-02-title-bar.png`, mobile ? '.mobile-thread-title-row' : '.thread-view-header',
    'Thread title bar', mobile ? 'MobileThreadTitleBar' : 'ThreadView .thread-view-header');
  await shot(`${prefix}-03-composer.png`, '.prompt-area', 'Composer', 'PromptInput + PromptRowControls');

  // A tall viewport, so a long turn fits one element shot whole.
  const size = page.viewportSize();
  if (size) await page.setViewportSize({ width: size.width, height: 2400 });
  for (const [i, scene] of seeded.scenes.entries()) {
    const exchange = `.chat-exchange[data-event-id="${scene.anchorId}"]`;
    await shotTurns(page, `${dir}/${prefix}-${String(i + 10).padStart(2, '0')}-${scene.slug}.png`, scene.eventIds);
    written.push({ file: `${prefix}-${String(i + 10).padStart(2, '0')}-${scene.slug}.png`, title: scene.title, trigger: scene.trigger, thread: seeded.title });
    for (const [kind, selector] of HEADER_KINDS) {
      const rows = page.locator(`${exchange} ${selector}`).filter({ visible: true });
      for (let n = 0; n < await rows.count(); n++) {
        const file = `${prefix}-hdr-${scene.slug}-${kind}-${n}.png`;
        await rows.nth(n).screenshot({ path: `${dir}/${file}`, ...opts });
        written.push({ file, title: scene.title, trigger: scene.trigger, thread: seeded.title, header: kind });
      }
    }
  }
  if (size) await page.setViewportSize(size);

  // The surfaces a thread opens over itself, each with its own head.
  const popover = async (file: string, opener: string, panel: string, title: string, trigger: string) => {
    const open = page.locator(opener).filter({ visible: true }).first();
    if (await open.count() === 0) return;
    await open.click();
    const target = page.locator(panel).filter({ visible: true }).first();
    const opened = await target.waitFor({ state: 'visible', timeout: 5_000 }).then(() => true, () => false);
    if (!opened) return;
    await target.screenshot({ path: `${dir}/${file}`, ...opts });
    written.push({ file, title, trigger, thread: seeded.title });
    await page.keyboard.press('Escape');
    await expect(target).toBeHidden();
  };
  await popover(`${prefix}-40-route-panel.png`, '.response-executor', '.message-route-panel',
    'Route popover (tap the actor chip)', 'MessageRoutePanel');
  await popover(`${prefix}-41-step-detail.png`, '.inline-step .step-main', '.step-detail-modal',
    'Step detail (tap a step)', 'StepDetailModal');
  await popover(`${prefix}-42-todo-panel.png`, '[data-role="todo-indicator"]', '.todo-panel',
    'Todo list (composer indicator)', 'TodoListPanel');
  return written;
}

test.describe('Thread shots', () => {
  test.skip(!SHOOTING, 'set THREAD_SHOTS=1 to take screenshots');
  test.setTimeout(240_000);

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('every kind of turn, in both thread kinds', async ({ context }, testInfo) => {
    const dir = `${ROOT}/${testInfo.project.name}`;
    mkdirSync(dir, { recursive: true });
    const chat = seedThread({ title: 'Plan the Q4 offsite', source: 'chat', scenes: chatScenes() });
    const changeId = randomUUID();
    const cc = seedThread({ title: 'Dark-mode toggle', source: 'claude_code', scenes: ccScenes(changeId) });
    seedAppliedChange(changeId, cc.threadId);

    const entries = [
      ...await shootThread(context, dir, 'chat', chat),
      ...await shootThread(context, dir, 'cc', cc),
    ];
    writeFileSync(`${dir}/scenes.json`, JSON.stringify(entries, null, 2));
    expect(entries.length).toBeGreaterThan(0);
  });
});
