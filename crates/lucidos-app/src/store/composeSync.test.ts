import { afterEach, expect, it } from 'vitest';
import { acceptComposeReport, serverComposeOf, noteLocalComposeEdit, acknowledgeComposeEdits, isComposeIntentPending, _resetComposeSyncForTesting } from './composeSync';
import { getDraft, setDraft, _resetComposeDraftsForTesting } from './composeDrafts';
import { installRestoreAdmission } from '../utils/restoreAdmission';
import { patchComposeSelection, getComposeSelectionOverride } from './composeSelections';

const report = { revision: 3, epoch: 1, text: 'server draft', imageHashes: [], mode: null, selection: null };

afterEach(() => {
  installRestoreAdmission({ ready: () => true, phase: () => 'admitted', wait: async () => {} });
  _resetComposeDraftsForTesting();
  _resetComposeSyncForTesting();
});

const restored = { ...report, revision: 5, epoch: 2, text: 'committed edit\n\nqueued',
  imageHashes: ['committed-image', 'queued-image'],
  restorations: [{ epoch: 2, text: 'queued', image_hashes: ['queued-image'] }] };

it('rebases exact additions over pending replacements once across reports and reconnects', () => {
  acceptComposeReport('pending', { ...report, revision: 1, text: 'saved', imageHashes: ['old'] });
  setDraft('pending', { text: 'local replacement', image_hashes: ['local'], mode: null });
  noteLocalComposeEdit('pending');
  acceptComposeReport('pending', restored);
  expect(getDraft('pending')).toMatchObject({ text: 'local replacement\n\nqueued', image_hashes: ['local', 'queued-image'] });
  acceptComposeReport('pending', { ...restored, revision: 6 });
  expect(getDraft('pending').text).toBe('local replacement\n\nqueued');
});

it('a delayed successful PUT cannot acknowledge restoration it did not carry', () => {
  acceptComposeReport('delayed', { ...report, revision: 1 });
  setDraft('delayed', { text: 'committed edit', image_hashes: ['committed-image'], mode: null });
  const sentSeq = noteLocalComposeEdit('delayed');
  acceptComposeReport('delayed', restored);
  acknowledgeComposeEdits('delayed', sentSeq, { ...report, revision: 4, text: 'committed edit' });
  expect(isComposeIntentPending('delayed')).toBe(true);
  expect(getDraft('delayed').text).toBe('committed edit\n\nqueued');
});

it('does not replay an addition included in the local editing baseline', () => {
  acceptComposeReport('baseline', restored);
  setDraft('baseline', { text: 'deliberately removed queued text', image_hashes: [], mode: null });
  noteLocalComposeEdit('baseline');
  acceptComposeReport('baseline', { ...restored, revision: 6 });
  expect(getDraft('baseline').text).toBe('deliberately removed queued text');
});

it('reconciles multiple missed restorations while preserving mode and selection', () => {
  acceptComposeReport('multiple', { ...report, revision: 1 });
  setDraft('multiple', { text: 'local', image_hashes: ['queued-image'], mode: 'lucidos' });
  patchComposeSelection('multiple', { model: 'local-model' });
  noteLocalComposeEdit('multiple');
  acceptComposeReport('multiple', { ...restored, revision: 6, epoch: 3,
    restorations: [...restored.restorations, { epoch: 3, text: 'second', image_hashes: ['second-image'] }] });
  expect(getDraft('multiple')).toEqual({ text: 'local\n\nqueued\n\nsecond', image_hashes: ['queued-image', 'second-image'], mode: 'lucidos' });
  expect(getComposeSelectionOverride('multiple').model).toBe('local-model');
});

it('a device with no pending edits adopts the stored result without appending', () => {
  acceptComposeReport('peer', restored);
  acceptComposeReport('peer', { ...restored, revision: 6 });
  expect(getDraft('peer').text).toBe('committed edit\n\nqueued');
  expect(isComposeIntentPending('peer')).toBe(false);
});

it('a consumed addition is not resurrected by a late report', () => {
  acceptComposeReport('consumed', { ...report, revision: 1 });
  setDraft('consumed', { text: 'new edit', image_hashes: [], mode: null });
  noteLocalComposeEdit('consumed');
  acceptComposeReport('consumed', { ...report, revision: 7, epoch: 3, text: '', restorations: [] });
  acceptComposeReport('consumed', restored);
  expect(getDraft('consumed').text).toBe('new edit');
});

// Restore capture reads the composer. A provisional read before admission
// must not put server text there, or a restore would capture it as intent.
it('ignores a compose report heard before restore admission', () => {
  installRestoreAdmission({ ready: () => false, phase: () => 'generation', wait: () => new Promise(() => {}),
    provisional: () => true });
  expect(acceptComposeReport('before-admission', report)).toBe(false);
  expect(getDraft('before-admission').text).toBe('');
  expect(serverComposeOf('before-admission')).toBeUndefined();
});

it('applies the same report once admitted', () => {
  expect(acceptComposeReport('after-admission', report)).toBe(true);
  expect(getDraft('after-admission').text).toBe('server draft');
});

// A reconnect closes admission again. A PUT's answer read then is this page's
// own write, and dropping it would draw an older server draft over it.
it('applies a report heard while a reconnect awaits admission', () => {
  installRestoreAdmission({ ready: () => false, phase: () => 'awaiting-hello', wait: () => new Promise(() => {}),
    provisional: () => false });
  expect(acceptComposeReport('reconnecting', report)).toBe(true);
  expect(getDraft('reconnecting').text).toBe('server draft');
});
