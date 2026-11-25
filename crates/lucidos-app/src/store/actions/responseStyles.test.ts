import { describe, it, expect, beforeEach, vi } from 'vitest';
import { responseStyles, toasts } from '../store';

const { listResponseStyles, setPreference } = vi.hoisted(() => ({
  listResponseStyles: vi.fn(),
  setPreference: vi.fn(),
}));

vi.mock('../../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../api/client')>();
  return { ...actual, listResponseStyles, setPreference };
});

const { loadResponseStyles, saveStyleDocument } = await import('./responseStyles');

const TERSE = { id: 'terse', name: 'Terse', instructions: 'Be brief.' } as never;

beforeEach(() => {
  vi.clearAllMocks();
  toasts.value = [];
  responseStyles.value = { status: 'not-loaded' };
});

describe('loadResponseStyles', () => {
  it('a failed refetch keeps the rows already on screen', async () => {
    responseStyles.value = { status: 'loaded', data: [TERSE] };
    listResponseStyles.mockRejectedValueOnce(new Error('Network down'));

    await loadResponseStyles();

    expect(responseStyles.value).toEqual({ status: 'loaded', data: [TERSE] });
  });
});

describe('saveStyleDocument', () => {
  it('a refused write reads as not saved and says why', async () => {
    listResponseStyles.mockResolvedValue({ styles: [] });
    setPreference.mockRejectedValueOnce(new Error('instructions too long'));

    const saved = await saveStyleDocument(() => [TERSE]);

    expect(saved, 'a refusal must not report the document as saved').toBe(false);
    expect(toasts.value.some((t) => t.message.includes('instructions too long'))).toBe(true);
  });
});
