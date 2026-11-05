/** Every backend a model route can name, with how the UI names it. Mirrors
 *  `llm::model_registry::ProviderKind`, whose `as_str` gives the values. */
export const PROVIDERS = [
  { value: 'anthropic', label: 'Anthropic (direct)' },
  { value: 'vertex', label: 'Vertex' },
  { value: 'openai', label: 'OpenAI' },
  { value: 'openrouter', label: 'OpenRouter' },
  { value: 'xai', label: 'xAI' },
  { value: 'opencode-free', label: 'OpenCode Free (keyless)' },
  { value: 'local', label: 'Local (OpenAI-compatible)' },
  { value: 'claude-gateway', label: 'Claude gateway' },
];

/** How the UI names a provider, falling back to the raw value. */
export function providerLabel(provider: string): string {
  return PROVIDERS.find((p) => p.value === provider)?.label ?? provider;
}

