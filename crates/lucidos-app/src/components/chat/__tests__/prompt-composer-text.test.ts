/**
 * The composer sends the draft it is showing.
 *
 * The Send face is rendered from the draft store, and `sendCompose` sends the
 * draft too. The textarea shows the draft after every render, so the submit
 * paths read the draft alone. A button lit by one value and refused by another
 * is a dead press with nothing on screen.
 *
 * Plans: docs/plans/2026-08-27-the-composer-sends-the-draft-it-is-showing.md,
 * docs/plans/2026-10-12-compose-draft-follows-a-server-revision.md
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

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

describe('the submit paths read the draft, the one copy of the message', () => {
  it('submit sends the draft', () => {
    const body = submitBody();
    expect(body).toMatch(/const draftText = getDraft\(threadId\)\.text/);
    expect(body).toMatch(/const msg = draftText\.trim\(\)/);
  });

  it('submit no longer returns on a missing textarea node', () => {
    expect(submitBody()).not.toMatch(/if\s*\(\s*!el\s*\)\s*return/);
  });

  it('submit leaves no silent return at all', () => {
    // Every return owes the user a word. A press that produces neither a
    // message nor a toast is the whole bug. It stays quiet only for Enter on a
    // genuinely empty desktop composer. A side question speaks through its
    // card, and a send held or refused by an image upload through the
    // composer's upload line (`uploadSendNotice`). A send the rung switch
    // check cancels speaks through its dialog or its toast.
    const before = submitBody().split(/\breturn;/).slice(0, -1);
    const speaks = /showToast\(|ask\w*SideQuestion\(|queueUploadSend\(|markUploadBlockedSend\(|passSendGate\(/;
    const silent = before.filter((seg) => !speaks.test(seg.slice(-400)));
    expect(silent).toHaveLength(0);
  });

  it('submit dispatches on the SAME reading the Send face was lit from', () => {
    expect(submitBody()).toMatch(
      /composeHasContent\(msg, currentImages\.length, uploadInFlight\)/,
    );
    expect(promptSource).toMatch(
      /const hasContent = composeHasContent\(composeText, images\.length, uploadsBlocking\)/,
    );
  });

  it('the multi answer reads the source its own count came from', () => {
    expect(promptSource).toContain('computeSubmitMultiCount(multiSelectedIds.length, composeText)');
    expect(submitMultiBody()).toMatch(/const text = getDraft\(focused\)\.text\.trim\(\)/);
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
