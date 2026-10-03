/**
 * The composer's side-question route (ADR 0320). In side-question mode, or
 * from the Send button's hold, the box goes to the side-question endpoint and
 * never to `sendFollowup`. Everything else still reaches the normal send.
 *
 * Source-pattern checks for PromptInput, as its sibling tests do: mounting it
 * drags in every chat signal. The routing decision itself is unit-tested in
 * `store/sideQuestions.test.ts`, and the browser flow in
 * `e2e/side-question.spec.ts`.
 */
import { describe, expect, it } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const promptSource = readFileSync(resolve(here, '../PromptInput.tsx'), 'utf-8');
const menuSource = readFileSync(resolve(here, '../CodingAgentControlMenu.tsx'), 'utf-8');

describe('PromptInput submit routes a side question before any send', () => {
  const submit = promptSource.match(/async function submit\(asSideQuestion = false\)[\s\S]*?\n {2}\}/)?.[0] ?? '';

  it('decides the route ahead of the upload queue and the send', () => {
    expect(submit, 'submit() not found').not.toBe('');
    const routeIdx = submit.indexOf('routeSideQuestion(');
    expect(routeIdx).toBeGreaterThan(-1);
    expect(submit.indexOf('queueUploadSend')).toBeGreaterThan(routeIdx);
    expect(submit.indexOf('beginSend')).toBeGreaterThan(routeIdx);
  });

  // In the mode the box asks, so a multi-select card stops taking its text and
  // the lone Submit reads Ask.
  it('lets side-question mode win over a waiting card', () => {
    expect(promptSource).toMatch(/const hasPendingMultiQ = pendingMultiQ !== null && !sideQuestionMode;/);
    expect(promptSource).toContain("{sideQuestionMode ? 'Ask' : 'Submit'}");
  });

  it('asks in side-question mode as well as from the hold', () => {
    expect(submit).toMatch(/routeSideQuestion\(msg, \{[\s\S]*?\}, asSideQuestion \|\| sideQuestionMode\)/);
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

  // The emptied box turns the button into Stop or a card's Cancel at once.
  it('asking arms the settle window, so a repeat tap cannot cancel', () => {
    const helper = promptSource.match(/function askComposerSideQuestion\([\s\S]*?\n {2}\}/)?.[0] ?? '';
    expect(helper.indexOf('armCancelSettle()')).toBeGreaterThan(-1);
    expect(helper.indexOf('armCancelSettle()')).toBeLessThan(helper.indexOf("updateCompose(threadId, { text: '', image_hashes: [] })"));
  });

  it('asking turns side-question mode off', () => {
    const helper = promptSource.match(/function askComposerSideQuestion\([\s\S]*?\n {2}\}/)?.[0] ?? '';
    expect(helper).toContain('updateComposeSelection(threadId, { sideQuestionMode: false })');
  });

  it('a queued ask routes as a side question once the upload lands', () => {
    const queued = promptSource.match(/function sendQueuedAfterUpload\([\s\S]*?\n {2}\}/)?.[0] ?? '';
    expect(queued).toMatch(/routeSideQuestion\(msg, \{\s*started: thread\.meta\.state !== 'composing',[\s\S]*?\}, intent\.asSideQuestion === true\)/);
    expect(queued.indexOf('routeSideQuestion(')).toBeLessThan(queued.indexOf('beginSend('));
  });

  it('a refusal toasts and returns, keeping the draft', () => {
    const branch = submit.match(/if \(sideQuestion\.kind === 'refuse'\) \{[\s\S]*?\n {4}\}/)?.[0] ?? '';
    expect(branch).toContain('showToast(sideQuestion.toast');
    expect(branch).toContain('return;');
    expect(branch).not.toContain('updateCompose');
  });
});

// A typed `/btw` is ordinary text, and nothing hands it back as a side question.
describe('nothing treats /btw as special', () => {
  it('the command menu neither lists nor intercepts it', () => {
    expect(menuSource).not.toMatch(/btw/i);
  });

  it('the composer never writes it into the box', () => {
    expect(promptSource).not.toMatch(/btw/i);
  });
});
