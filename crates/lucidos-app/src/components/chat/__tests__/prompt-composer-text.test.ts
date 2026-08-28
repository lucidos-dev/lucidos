/**
 * The composer sends the draft it is showing.
 *
 * The Send face is rendered from the draft store, and `sendCompose` sends the
 * draft too. The submit paths used to gate on the textarea instead. A button lit
 * by one value was then refused by another the moment the two drifted apart.
 * That is a dead press with nothing on screen, which is how four reports of a
 * dead composer button read.
 *
 * Plan: docs/plans/2026-08-27-the-composer-sends-the-draft-it-is-showing.md
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { resolveComposerText, composerTextDisagreementToast } from '../prompt-input-helpers';

describe('resolveComposerText: one source for what a submit sends', () => {
  it('sends the agreed text and reports nothing', () => {
    expect(resolveComposerText('hello', 'hello')).toEqual({
      text: 'hello', storeWrite: null, disagreed: false,
    });
  });

  it('sends nothing on an empty composer, which is the legitimate no-op', () => {
    expect(resolveComposerText('', '')).toEqual({
      text: '', storeWrite: null, disagreed: false,
    });
  });

  it('sends the draft when there is no textarea node at all', () => {
    // The old `if (!el) return` was a dead button that said nothing. A missing
    // node is an absent source, not a disagreement.
    expect(resolveComposerText('hello', null)).toEqual({
      text: 'hello', storeWrite: null, disagreed: false,
    });
  });

  it('sends the draft when the textarea is empty, and says so', () => {
    // The reported shape: the Send face is lit from the draft and the old gate
    // read the box, so the press did nothing at all.
    expect(resolveComposerText('hello', '')).toEqual({
      text: 'hello', storeWrite: null, disagreed: true,
    });
  });

  it('prefers the draft when both hold text, since the face was lit from it', () => {
    expect(resolveComposerText('draft text', 'box text')).toEqual({
      text: 'draft text', storeWrite: null, disagreed: true,
    });
  });

  it('recovers text the store lacks rather than refusing the send', () => {
    // `sendCompose` re-reads the store, so the caller has to put it there.
    expect(resolveComposerText('', 'typed in the box')).toEqual({
      text: 'typed in the box', storeWrite: 'typed in the box', disagreed: true,
    });
  });

  it('hands the store the RAW value, altering nothing the user typed', () => {
    const r = resolveComposerText('', '  two lines\nkept  ');
    expect(r.storeWrite).toBe('  two lines\nkept  ');
    expect(r.text).toBe('two lines\nkept');
  });

  it('never reads trailing whitespace as a disagreement', () => {
    // Both paths already trim what they send, so a trailing newline in one copy
    // must not fire the report.
    expect(resolveComposerText('hello\n', 'hello')).toEqual({
      text: 'hello', storeWrite: null, disagreed: false,
    });
  });
});

describe('composerTextDisagreementToast: which copy was sent', () => {
  it('says nothing when the two agreed', () => {
    expect(composerTextDisagreementToast(resolveComposerText('hello', 'hello'))).toBeNull();
  });

  it('names the draft, because the box may be showing the other one', () => {
    expect(composerTextDisagreementToast(resolveComposerText('hello', '')))
      .toContain('Sent the saved draft');
  });

  it('names the screen when the store was the empty one', () => {
    expect(composerTextDisagreementToast(resolveComposerText('', 'typed')))
      .toContain('Sent the text on screen');
  });
});

const here: string = dirname(fileURLToPath(import.meta.url));
const promptSource = readFileSync(resolve(here, '../PromptInput.tsx'), 'utf-8');

function submitBody(): string {
  const fn = promptSource.match(/async function submit\(\)[\s\S]*?\n {2}\}/);
  expect(fn, 'submit() not found').not.toBeNull();
  return fn![0];
}

function submitMultiBody(): string {
  const fn = promptSource.match(/async function submitMultiAnswer\(\)[\s\S]*?\n {2}\}/);
  expect(fn, 'submitMultiAnswer() not found').not.toBeNull();
  return fn![0];
}

describe('the submit paths are wired to that one source', () => {
  it('submit resolves the draft against the textarea', () => {
    const body = submitBody();
    expect(body).toMatch(/const draftText = getDraft\(threadId\)\.text/);
    expect(body).toMatch(/resolveComposerText\(draftText,\s*el \? el\.value : null\)/);
  });

  it('submit no longer returns on a missing textarea node', () => {
    expect(submitBody()).not.toMatch(/if\s*\(\s*!el\s*\)\s*return/);
  });

  it('submit leaves no silent return at all', () => {
    // Every return owes the user a word. A press that produces neither a
    // message nor a toast is the whole bug. The last one to fall was the empty
    // composer. It now speaks whenever the box holds characters, and stays
    // quiet only for Enter on a genuinely empty desktop composer.
    const before = submitBody().split(/\breturn;/).slice(0, -1);
    const silent = before.filter((seg) => !/showToast\(/.test(seg.slice(-400)));
    expect(silent).toHaveLength(0);
  });

  it('submit dispatches on the SAME reading the Send face was lit from', () => {
    // Two readings of "is there anything to send" is what an enabled Send
    // whose press does nothing is made of. See `composeHasContent`.
    expect(submitBody()).toMatch(
      /composeHasContent\(msg, currentImages\.length, uploadInFlight\)/,
    );
    expect(promptSource).toMatch(
      /const hasContent = composeHasContent\(composeText, images\.length, uploadsBlocking\)/,
    );
  });

  it('submit repairs the store BEFORE any dispatch reads it back', () => {
    // A queued upload send re-reads the draft later and `sendCompose` re-reads
    // it now. Either would otherwise carry the empty copy.
    const body = submitBody();
    const write = body.indexOf('updateCompose(threadId, { text: resolved.storeWrite })');
    expect(write).toBeGreaterThan(-1);
    expect(body.indexOf('queueUploadSend(threadId')).toBeGreaterThan(write);
    expect(body.indexOf('beginSend')).toBeGreaterThan(write);
  });

  it('the multi answer reads the source its own count came from', () => {
    expect(promptSource).toContain('computeSubmitMultiCount(multiSelectedIds.length, composeText)');
    expect(submitMultiBody()).toMatch(
      /resolveComposerText\(getDraft\(focused\)\.text,\s*el \? el\.value : null\)/,
    );
  });

  it('both paths report a disagreement rather than passing it on', () => {
    expect(submitBody()).toContain('composerTextDisagreementToast');
    expect(submitMultiBody()).toContain('composerTextDisagreementToast');
  });

  it('says nothing about a send with no thread to hold a draft', () => {
    // A raw-new send has no stored copy, so the box is the only source and
    // there is nothing for it to disagree with.
    expect(submitBody()).toMatch(/threadId \? composerTextDisagreementToast\(resolved\) : null/);
  });
});

describe('sendCompose never returns without sending and without saying so', () => {
  const composeSource = readFileSync(
    resolve(here, '../../../store/actions/compose.ts'),
    'utf-8',
  );
  const fn = composeSource.match(/export async function sendCompose\([\s\S]*?\n\}/);

  it('speaks when the thread is gone', () => {
    expect(fn![0]).toMatch(/if \(!thread\) \{\s*\n\s*showToast\(/);
  });

  it('speaks when the stored draft is empty', () => {
    expect(fn![0]).toMatch(/wireHashes\.length === 0\) \{\s*\n\s*showToast\(/);
  });
});
