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
  { value: 'chatgpt-plan', label: 'ChatGPT plan' },
];

/** The Providers page as copy names it. */
export const PROVIDERS_SETTINGS_PATH = 'Settings → Providers';

/** The search and deep-link anchor of a provider's block on the Providers page.
 *  Takes a `PROVIDERS` value or a System One provider id, and a suffix for a row
 *  inside the block (`providerAnchor('vertex', 'region')`). */
export function providerAnchor(provider: string, row?: string): string {
  return row ? `providers:${provider}-${row}` : `providers:${provider}`;
}

/** How the UI names a provider, falling back to the raw value. */
export function providerLabel(provider: string): string {
  return PROVIDERS.find((p) => p.value === provider)?.label ?? provider;
}

