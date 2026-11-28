export const PROVIDERS = [
  { value: 'anthropic', label: 'Anthropic (direct)' },
  { value: 'vertex', label: 'Vertex' },
  { value: 'openai', label: 'OpenAI' },
  { value: 'openrouter', label: 'OpenRouter' },
  { value: 'xai', label: 'xAI' },
  { value: 'opencode-free', label: 'OpenCode Free (keyless)' },
  { value: 'local', label: 'Local (OpenAI-compatible)' },
  { value: 'claude-gateway', label: 'Claude gateway' },
  { value: 'chatgpt-plan', label: 'ChatGPT plan' },
];

export const PROVIDERS_SETTINGS_PATH = 'Settings → Providers';

export function providerAnchor(provider: string, row?: string): string {
  return row ? `providers:${provider}-${row}` : `providers:${provider}`;
}

export function providerLabel(provider: string): string {
  return PROVIDERS.find((p) => p.value === provider)?.label ?? provider;
}

