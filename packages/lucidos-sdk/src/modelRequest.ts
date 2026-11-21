import { request } from './request';
import { callableRoute, modelChoices, resolveSelection, type RegistryModel, type ModelPickerValue, type ModelProxyProtocol } from './_modelRegistry';

export interface ModelMessage {
  role: 'user' | 'assistant';
  content: string;
}
export interface ModelRequestOptions {
  messages: readonly ModelMessage[];
  system?: string;
  stream?: boolean;
  maxTokens?: number;
}
export interface ModelRequest {
  proxy: string;
  protocol: ModelProxyProtocol;
  path: string;
  headers: Record<string, string>;
  body: Record<string, unknown>;
}

/** Build a request without sending it or exposing credentials. */
export async function modelRequest(value: ModelPickerValue, options: ModelRequestOptions): Promise<ModelRequest> {
  const registry = await request<{ models: RegistryModel[] }>('/models');
  const selection = resolveSelection(modelChoices(registry.models, { callableOnly: true }), value);
  const model = registry.models.find(row => row.id === selection.model)!;
  const route = model.routes.find(row => row.provider === selection.provider)!;
  if (!callableRoute(route) || !route.proxy) throw new Error('Route has no supported builtin proxy');
  const { protocol, name, request_defaults } = route.proxy;
  const defaults = request_defaults?.[selection.reasoning_effort ?? ''];
  if (!defaults) throw new Error('Model request metadata is unavailable; refresh Lucidos and retry');
  const body: Record<string, unknown> = { ...defaults.body };
  const headers = { 'content-type': 'application/json', ...defaults.headers };
  const stream = options.stream ?? false;
  const effort = selection.reasoning_effort;
  let path: string;
  switch (protocol) {
    case 'anthropic-messages':
    case 'vertex-anthropic': {
      const wire = route.id.replace(/\[1m\]$/, '');
      if (protocol === 'vertex-anthropic') {
        path = `/publishers/anthropic/models/${encodeURIComponent(wire)}:${stream ? 'streamRawPredict' : 'rawPredict'}`;
      } else {
        path = '/messages';
      }
      body.messages = options.messages.map(message => ({ ...message }));
      body.stream = stream;
      if (options.system !== undefined) body.system = options.system;
      if (options.maxTokens !== undefined) body.max_tokens = options.maxTokens;
      break;
    }
    case 'vertex-google':
      path = `/publishers/google/models/${encodeURIComponent(route.id)}:${stream ? 'streamGenerateContent?alt=sse' : 'generateContent'}`;
      body.contents = options.messages.map(message => ({
        role: message.role === 'assistant' ? 'model' : 'user', parts: [{ text: message.content }],
      }));
      if (options.system !== undefined) body.systemInstruction = { parts: [{ text: options.system }] };
      if (options.maxTokens !== undefined) body.generationConfig = {
        ...(body.generationConfig as Record<string, unknown> | undefined), maxOutputTokens: options.maxTokens,
      };
      break;
    case 'openai-responses':
      path = '/responses';
      Object.assign(body, { model: route.id, input: options.messages.map(message => ({ ...message })), stream, store: false });
      if (options.system !== undefined) body.instructions = options.system;
      if (effort !== null) body.reasoning = { effort };
      if (options.maxTokens !== undefined) body.max_output_tokens = options.maxTokens;
      break;
    case 'openai-chat-completions':
      path = '/chat/completions';
      Object.assign(body, { model: route.id, messages: [
        ...(options.system === undefined ? [] : [{ role: 'system', content: options.system }]),
        ...options.messages.map(message => ({ ...message })),
      ], stream });
      if (effort !== null) body.reasoning_effort = effort;
      if (options.maxTokens !== undefined) body.max_tokens = options.maxTokens;
      break;
    default:
      throw new Error(`Unsupported model request protocol: ${protocol}`);
  }
  return { proxy: name, protocol, path, headers, body };
}
