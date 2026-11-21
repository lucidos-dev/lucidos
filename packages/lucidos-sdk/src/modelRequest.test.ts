import { describe, expect, it, vi } from 'vitest';
import { modelRequest } from './modelRequest';
import { request } from './request';
import type { RegistryModel, ModelProxyProtocol } from './_modelRegistry';
vi.mock('./request', () => ({ request: vi.fn() }));
function registry(protocol: ModelProxyProtocol, provider: string, defaults = {}) {
  const row: RegistryModel = { id: 'alias', label: 'Alias', preferred_provider: null,
    successor: null, on_chat_ladder: true, default_effort: null, routes: [{
      provider, id: 'wire-model', configured: true, reasoning_efforts: ['low', 'high'],
      proxy: { name: provider, protocol, request_defaults: {
        '': { body: defaults, headers: {} }, high: { body: defaults, headers: { 'test-header': 'from-engine' } },
      } },
    }] };
  vi.mocked(request).mockResolvedValue({ models: [row] });
}
const value = { model: 'alias', reasoning_effort: null, provider: null };
const options = { messages: [{ role: 'user' as const, content: 'Hello' }], system: 'Be concise' };
describe('modelRequest wire contracts', () => {
  it.each([
    ['anthropic-messages', 'anthropic', '/messages'],
    ['anthropic-messages', 'claude-gateway', '/messages'],
    ['openai-responses', 'openai', '/responses'],
    ['openai-chat-completions', 'openai', '/chat/completions'],
    ['openai-chat-completions', 'openrouter', '/chat/completions'],
    ['openai-chat-completions', 'xai', '/chat/completions'],
    ['openai-chat-completions', 'local', '/chat/completions'],
    ['vertex-anthropic', 'vertex', '/publishers/anthropic/models/wire-model:rawPredict'],
    ['vertex-google', 'vertex', '/publishers/google/models/wire-model:generateContent'],
  ] as const)('builds %s through %s', async (protocol, provider, path) => {
    registry(protocol, provider);
    const result = await modelRequest(value, options);
    expect(result.proxy).toBe(provider);
    expect(result.protocol).toBe(protocol);
    expect(result.path).toBe(path);
    if (!protocol.startsWith('vertex')) {
      if (protocol !== 'anthropic-messages') expect(result.body.model).toBe('wire-model');
      expect(result.body.reasoning_effort).toBeUndefined();
      expect(result.body.reasoning).toBeUndefined();
    }
  });
  it('preserves engine-owned thinking settings and headers', async () => {
    registry('anthropic-messages', 'anthropic', { thinking: { type: 'adaptive' }, output_config: { effort: 'high' }, max_tokens: 9000 });
    const result = await modelRequest({ ...value, reasoning_effort: 'high' }, options);
    expect(result.body.thinking).toEqual({ type: 'adaptive' });
    expect(result.body.max_tokens).toBe(9000);
    expect(result.headers['test-header']).toBe('from-engine');
  });
  it('translates Google contents and preserves engine generation config', async () => {
    registry('vertex-google', 'vertex', { generationConfig: { thinkingConfig: { thinkingLevel: 'high' } } });
    const result = await modelRequest(value, { ...options, maxTokens: 100, stream: true });
    expect(result.body.contents).toEqual([{ role: 'user', parts: [{ text: 'Hello' }] }]);
    expect(result.body.generationConfig).toEqual({ thinkingConfig: { thinkingLevel: 'high' }, maxOutputTokens: 100 });
    expect(result.path).toContain(':streamGenerateContent?alt=sse');
  });
  it('rejects unavailable providers and missing request metadata', async () => {
    registry('openai-responses', 'openai');
    await expect(modelRequest({ ...value, provider: 'local' }, options)).rejects.toThrow('Provider is unavailable');
    await expect(modelRequest({ ...value, reasoning_effort: 'low' }, options)).rejects.toThrow('metadata is unavailable');
  });
});

it.each(['openai-responses', 'openai-chat-completions'] as const)('uses route effort verbatim for %s', async protocol => {
  registry(protocol, 'openai');
  const result = await modelRequest({ ...value, reasoning_effort: 'max' }, options);
  // The offered route caps at high; use the host's shared clamp before wiring.
  if (protocol === 'openai-responses') expect(result.body.reasoning).toEqual({ effort: 'high' });
  else expect(result.body.reasoning_effort).toBe('high');
});

it('retains null-effort defaults for a model that thinks by default', async () => {
  registry('anthropic-messages', 'anthropic', { model: 'wire-model', thinking: { type: 'adaptive' }, max_tokens: 9000 });
  const result = await modelRequest(value, options);
  expect(result.body.thinking).toEqual({ type: 'adaptive' });
  expect(result.body.model).toBe('wire-model');
});
