import { describe, it, expect, beforeEach, vi } from 'vitest';
import { panelOverlay, activeInlineForm, connectionStatus } from '../store';
import type { CredentialRequest, EmailConfirmRequest } from '../types';

// Mock all external dependencies so handleResume can run in isolation
vi.mock('../../api/client', () => ({
  checkHealth: vi.fn().mockResolvedValue({ workspace: 'test', workspace_path: '/tmp/test' }),
  API_BASE: 'http://localhost:3000',
}));
vi.mock('./thread-sync', () => ({
  connectThreadEvents: vi.fn(),
  disconnectThreadEvents: vi.fn(),
}));
vi.mock('./thread-loading', () => ({
  loadAllThreads: vi.fn().mockResolvedValue(undefined),
  refreshThreadEvents: vi.fn().mockResolvedValue(undefined),
  clearForcedRetries: vi.fn(),
}));
vi.mock('./chat-changes', () => ({
  refreshChangesState: vi.fn(),
  RESTART_LS_KEY: 'restart-required',
}));
vi.mock('./notifications', () => ({
  refreshUnreadCount: vi.fn(),
}));

// Import after mocks are set up
const { handleResume } = await import('./connection');

const emailConfirmForm = {
  type: 'email-confirm' as const,
  request: {
    to: ['test@example.com'],
    subject: 'Test',
    body: 'Hello',
    account: 'work',
    from: 'me@example.com',
  } as EmailConfirmRequest,
};

beforeEach(() => {
  panelOverlay.value = null;
  connectionStatus.value = 'connected';
});

describe('handleResume preserves email-confirm form', () => {
  it('should NOT clear email-confirm form on resume/focus', async () => {
    panelOverlay.value = { type: 'form', form: emailConfirmForm };

    await handleResume();

    expect(activeInlineForm.value).not.toBeNull();
    expect(activeInlineForm.value?.type).toBe('email-confirm');
  });

  it('should preserve the full email draft data on resume', async () => {
    panelOverlay.value = { type: 'form', form: emailConfirmForm };

    await handleResume();

    const form = activeInlineForm.value;
    expect(form?.type).toBe('email-confirm');
    if (form?.type === 'email-confirm') {
      expect(form.request.to).toEqual(['test@example.com']);
      expect(form.request.subject).toBe('Test');
    }
  });
});

const credentialRequestForm = {
  type: 'credential' as const,
  request: {
    service: 'helius',
    base_url: 'https://api.helius.xyz',
    auth_type: 'api_key' as const,
    prompt: 'Paste your Helius API key.\n1. Go to https://dev.helius.xyz/dashboard\n2. Copy API Key',
  } as CredentialRequest,
};

describe('handleResume preserves credential request form', () => {
  it('should NOT clear credential request form on resume/focus', async () => {
    panelOverlay.value = { type: 'form', form: credentialRequestForm };

    await handleResume();

    // User often takes a screenshot, switches tabs, or alt-tabs while filling
    // out credentials — the panel must survive every focus event. The data
    // lives on panelOverlay (and is persisted in the nav stack), so resync
    // does not need to "refetch" it from the original SSE event.
    expect(activeInlineForm.value).not.toBeNull();
    expect(activeInlineForm.value?.type).toBe('credential');
  });

  it('should preserve the full credential request prompt and instructions on resume', async () => {
    panelOverlay.value = { type: 'form', form: credentialRequestForm };

    await handleResume();

    const form = activeInlineForm.value;
    expect(form?.type).toBe('credential');
    if (form?.type === 'credential') {
      expect(form.request?.service).toBe('helius');
      expect(form.request?.prompt).toContain('1. Go to https://dev.helius.xyz/dashboard');
      expect(form.request?.prompt).toContain('2. Copy API Key');
    }
  });
});
