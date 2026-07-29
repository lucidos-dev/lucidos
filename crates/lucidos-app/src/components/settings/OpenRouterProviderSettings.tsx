import { ApiKeyProviderSettings } from './ApiKeyProviderSettings';

/** Configure the OpenRouter provider credential (Settings → Models →
 *  Providers). Stores a credential named `openrouter`; the engine builds an
 *  OpenAI-compatible provider pointed at openrouter.ai and sends it as the
 *  bearer key (preferring it over the LUCIDOS_OPENROUTER_API_KEY launch env
 *  var). OpenRouter authenticates with a single API key, so — like OpenAI —
 *  there's no auth-kind choice; stored as `api_key`. */
export function OpenRouterProviderSettings() {
  return (
    <ApiKeyProviderSettings
      service="openrouter"
      baseUrl="https://openrouter.ai/api/v1"
      label="OpenRouter"
      placeholder="sk-or-…"
      note={
        <>
          OpenRouter serves models on the <strong>openrouter</strong> provider (e.g. GLM 5.2).
          Stored here, the key is used instead of the <strong>LUCIDOS_OPENROUTER_API_KEY</strong> launch
          environment variable, which stays as a fallback. Adding the credential takes effect on the next
          engine restart.
        </>
      }
    />
  );
}
