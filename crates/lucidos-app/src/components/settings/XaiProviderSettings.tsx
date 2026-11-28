import { ApiKeyProviderSettings } from './ApiKeyProviderSettings';

export function XaiProviderSettings() {
  return (
    <ApiKeyProviderSettings
      service="xai"
      baseUrl="https://api.x.ai/v1"
      label="xAI"
      placeholder="xai-…"
      note={
        <>
          xAI serves the Grok models on the <strong>xai</strong> provider (e.g. Grok 4.6).
          Stored here, the key is used instead of the <strong>LUCIDOS_XAI_API_KEY</strong> launch
          environment variable, which stays as a fallback. Grok is also reachable through
          OpenRouter under an <strong>x-ai/</strong> prefixed id. The two are separate models in
          the picker and use separate keys.
        </>
      }
    />
  );
}
