import { ApiKeyProviderSettings } from './ApiKeyProviderSettings';

export function OpenAiProviderSettings() {
  return (
    <ApiKeyProviderSettings
      service="openai"
      baseUrl="https://api.openai.com"
      label="OpenAI (direct)"
      placeholder="sk-…"
      systemOne="openai"
      note={
        <>
          Direct OpenAI serves models on the <strong>openai</strong> provider (e.g. GPT-5). Stored here,
          the key is used instead of the <strong>OPENAI_API_KEY</strong> launch environment variable,
          which stays as a fallback when no key is set here.
          {' '}The same key answers judgments through <strong>OpenAI Decisions</strong>: the wake
          decision by default, and any judgment whose model control picks it.
        </>
      }
    />
  );
}
