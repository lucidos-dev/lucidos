import { PREF_CHAT_MODEL } from '@lucidos/preference-catalog';


export const DEFAULT_CHAT_MODEL: string = PREF_CHAT_MODEL.fallback;

export const MODELS = [
  { value: 'claude-fable-5-2', label: 'Fable 5.2' },
  { value: 'gemini-4-neon', label: 'Gemini 4 Neon' },
  { value: 'gemini-4-carbon', label: 'Gemini 4 Carbon' },
  { value: 'gemini-4-argon', label: 'Gemini 4 Argon' },
  { value: 'claude-fable-5-1', label: 'Fable 5.1' },
  { value: 'claude-opus-5-5', label: 'Opus 5.5' },
  { value: 'claude-sonnet-5-5', label: 'Sonnet 5.5' },
  { value: 'claude-haiku-5-5', label: 'Haiku 5.5' },
  { value: 'gemini-3.1-pro-preview', label: 'Gemini 3.1 Pro' },
  { value: 'gemini-3.8-flash', label: 'Gemini 3.8 Flash' },
  { value: 'gemini-3.1-flash-lite', label: 'Gemini 3.1 Flash-Lite' },
  { value: 'gpt-6.2-astra', label: 'GPT-6.2 Astra' },
  { value: 'gpt-6.2-sol', label: 'GPT-6.2 Sol' },
  { value: 'gpt-6-astra', label: 'GPT-6 Astra' },
  { value: 'gpt-6.1-sol', label: 'GPT-6.1 Sol' },
  { value: 'gpt-6-luna', label: 'GPT-6 Luna' },
];

export { PROVIDERS, providerLabel } from './modelProviders';

export { REASONING_LEVELS, availableReasoningLevels } from './reasoningLevels';
