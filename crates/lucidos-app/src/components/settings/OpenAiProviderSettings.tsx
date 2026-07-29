import { ApiKeyProviderSettings } from './ApiKeyProviderSettings';

/** Configure the direct-OpenAI provider credential (Settings → Models →
 *  Providers). Stores a credential named `openai`; the engine's OpenAiProvider
 *  reads it (preferring it over the OPENAI_API_KEY launch env var) and sends it
 *  as the bearer key. OpenAI authenticates with a single API key, so — unlike
 *  Anthropic — there's no auth-kind choice; the credential is stored as
 *  `api_key`. */
export function OpenAiProviderSettings() {
  return (
    <ApiKeyProviderSettings
      service="openai"
      baseUrl="https://api.openai.com"
      label="OpenAI (direct)"
      placeholder="sk-…"
      note={
        <>
          Direct OpenAI serves models on the <strong>openai</strong> provider (e.g. GPT-5). Stored here,
          the key is used instead of the <strong>OPENAI_API_KEY</strong> launch environment variable,
          which stays as a fallback when no key is set here.
        </>
      }
    />
  );
}
