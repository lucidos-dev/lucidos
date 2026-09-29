/**
 * The composer's `/btw` route (ADR 0318). A side question in a coding-agent
 * thread goes to the side-question endpoint and never to `sendFollowup`.
 * Everything else still reaches the normal send.
 *
 * Source-pattern checks for PromptInput, as its sibling tests do: mounting it
 * drags in every chat signal. The routing decision itself is unit-tested in
 * `store/sideQuestions.test.ts`, and the browser flow in
 * `e2e/side-question.spec.ts`.
 */
import { describe, expect, it, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

vi.mock('../../../store/actions/chat', () => ({ sendMessage: vi.fn() }));

import { withSideQuestionCommand } from '../CodingAgentControlMenu';

const here: string = dirname(fileURLToPath(import.meta.url));
const promptSource = readFileSync(resolve(here, '../PromptInput.tsx'), 'utf-8');

describe('PromptInput submit routes /btw before any send', () => {
  const submit = promptSource.match(/async function submit\(asSideQuestion = false\)[\s\S]*?\n {2}\}/)?.[0] ?? '';

  it('decides the route ahead of the upload queue and the send', () => {
    expect(submit, 'submit() not found').not.toBe('');
    const routeIdx = submit.indexOf('routeSideQuestion(');
    expect(routeIdx).toBeGreaterThan(-1);
    expect(submit.indexOf('queueUploadSend')).toBeGreaterThan(routeIdx);
    expect(submit.indexOf('beginSend')).toBeGreaterThan(routeIdx);
  });

  it('the ask branch asks with the images, or waits for them, and never sends', () => {
    const branch = submit.match(/if \(sideQuestion\.kind === 'ask'[\s\S]*?\n {4}\}/)?.[0] ?? '';
    expect(branch, 'ask branch not found').not.toBe('');
    expect(branch).toContain('askComposerSideQuestion(threadId, sideQuestion.question, currentImages)');
    expect(branch).toMatch(/if \(uploadInFlight\) \{[\s\S]*?asSideQuestion: true[\s\S]*?return;/);
    expect(branch).toMatch(/return;\s*\}$/);
    expect(branch).not.toMatch(/sendFollowup|sendMessage|sendCompose|beginSend/);
  });

  it('asking empties the draft, images included, and follows the new card', () => {
    const helper = promptSource.match(/function askComposerSideQuestion\([\s\S]*?\n {2}\}/)?.[0] ?? '';
    expect(helper, 'askComposerSideQuestion not found').not.toBe('');
    expect(helper).toContain("updateCompose(threadId, { text: '', image_hashes: [] })");
    // Before the ask, so it can tell the new card from the ones on screen.
    expect(helper.indexOf('followSideQuestion()')).toBeGreaterThan(-1);
    expect(helper.indexOf('followSideQuestion()')).toBeLessThan(helper.indexOf('askSideQuestion('));
    expect(helper).toContain('askSideQuestion(threadId, question, hashes)');
  });

  it('a queued ask routes as a side question once the upload lands', () => {
    const queued = promptSource.match(/function sendQueuedAfterUpload\([\s\S]*?\n {2}\}/)?.[0] ?? '';
    expect(queued).toMatch(/routeSideQuestion\(msg, \{\s*started: thread\.meta\.state !== 'composing',[\s\S]*?\}, intent\.asSideQuestion\)/);
    expect(queued.indexOf('routeSideQuestion(')).toBeLessThan(queued.indexOf('beginSend('));
  });

  it('a refusal toasts and returns, keeping the draft', () => {
    const branch = submit.match(/if \(sideQuestion\.kind === 'refuse'\) \{[\s\S]*?\n {4}\}/)?.[0] ?? '';
    expect(branch).toContain('showToast(sideQuestion.toast');
    expect(branch).toContain('return;');
    expect(branch).not.toContain('updateCompose');
  });
});

describe('the command menu offers /btw', () => {
  it('in a live Claude Code thread only', () => {
    expect(withSideQuestionCommand(['compact', 'context'], true)).toEqual(['btw', 'compact', 'context']);
    expect(withSideQuestionCommand(['compact'], false)).toEqual(['compact']);
  });

  it('once, should Claude Code ever register it itself', () => {
    expect(withSideQuestionCommand(['btw', 'compact'], true)).toEqual(['btw', 'compact']);
  });
});
