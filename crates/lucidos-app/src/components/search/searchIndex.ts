import { hasUpdateChannelChoice } from '../../utils/updateChannel';
import { enginePackaged, releaseCheck, aliasRetiredSettingsAnchor, type SettingsSubview } from '../../store/store';
import type { SearchResultItem } from '../../api/client';
import { SHORTCUT_DEFS, bindingSearchText, shortcutSearchAnchor } from '../../utils/shortcuts';
import { displayBinding, bindingFor } from '../../store/actions/keybindings';
import { isMobile } from '../../utils/viewport';
import { isIOS, isTauri } from '../../utils/platform';
import { WORKSPACE_ID } from '../../utils/basePath';
import {
  externalLinkTargetConfigurable,
  currentMemoryModule,
  currentStyleOverrides,
  VOICE_RESIDENT_SECTIONS,
} from '../../store/actions/preferences';
import { SYSTEM_ONE_PROVIDERS } from '../settings/judgmentBackend';
import { PROVIDERS_SETTINGS_PATH, providerAnchor } from '../../store/modelProviders';
import { rankByTitle } from './titleMatch';
import { voiceReachable } from '../../voice/socketUrl';

type Subview = Exclude<SettingsSubview, 'main'>;

interface SettingsSearchEntry {
  id: string;
  label: string;
  subview: Subview;
  path: string;
  anchor?: string;
  keywords?: string;
  mobileOnly?: boolean;
  packagedOnly?: boolean;
  iosPwaOnly?: boolean;
  iosOnly?: boolean;
  tauriOnly?: boolean;
  gatewayOnly?: boolean;
  when?: () => boolean;
}

const hasStyleOverrides = () => Object.keys(currentStyleOverrides()).length > 0;

const SETTINGS_SEARCH_INDEX: SettingsSearchEntry[] = [
  { id: 'providers', label: 'Providers', subview: 'providers', path: 'Settings', keywords: 'providers vertex anthropic openai openrouter xai grok opencode free keyless local gcloud gcp google api key direct credential gpt claude system one typesafe cloudflare' },
  { id: 'models', label: 'Models', subview: 'models', path: 'Settings' },
  { id: 'intelligence', label: 'Intelligence', subview: 'intelligence', path: 'Settings', keywords: 'intelligence ladder rung model effort default picker cost time band price prices lucidos agent claude code codex' },
  { id: 'permissions', label: 'Permissions', subview: 'permissions', path: 'Settings', keywords: 'permissions security command guard safety allowlist claude code lucidos agent bash python tools' },
  { id: 'mcp', label: 'MCP Servers', subview: 'mcp', path: 'Settings', keywords: 'mcp model context protocol server tools context cost tokens start stop remove disable allowlist' },
  { id: 'coding-agents', label: 'Coding Agents', subview: 'coding-agents', path: 'Settings', keywords: 'coding agent claude code codex binary path cli repository repositories git worktree' },
  { id: 'accounts', label: 'Accounts', subview: 'accounts', path: 'Settings' },
  { id: 'locale', label: 'Locale', subview: 'locale', path: 'Settings', keywords: 'language timezone region locale time zone' },
  { id: 'marketplaces', label: 'Marketplaces', subview: 'marketplaces', path: 'Settings', keywords: 'marketplace plugin catalog install source registry' },
  { id: 'access', label: 'Access', subview: 'access', path: 'Settings', keywords: 'mobile access remote phone tailscale tailnet connect url network bind lan pairing code pair device add' },
  { id: 'webhooks', label: 'Webhooks', subview: 'webhooks', path: 'Settings', keywords: 'webhook inbound hook endpoint github stripe slack signature hmac token funnel event' },
  { id: 'event-hooks', label: 'Event Hooks', subview: 'event-hooks', path: 'Settings', keywords: 'event hook hooks arm disarm armed changed remove gate guard stop refuse block script hook.toml' },
  { id: 'devices', label: 'Devices', subview: 'devices', path: 'Settings', keywords: 'device phone laptop push notifications rename remove last seen' },
  { id: 'users', label: 'Users', subview: 'users', path: 'Settings', keywords: 'people person operator team member teammate user email add remove offboard whose device', gatewayOnly: true },
  { id: 'devices:paired', label: 'Paired devices', subview: 'devices', path: 'Settings → Devices', anchor: 'devices:list', keywords: 'paired pairing device revoke unpair sign out cut off network access', gatewayOnly: true },
  { id: 'system', label: 'System', subview: 'system', path: 'Settings' },
  { id: 'appearance', label: 'Appearance & Behavior', subview: 'appearance', path: 'Settings', keywords: 'appearance interface behavior theme font scale links browser' },
  { id: 'keyboard-shortcuts', label: 'Keyboard Shortcuts', subview: 'keyboard-shortcuts', path: 'Settings', keywords: 'keybindings hotkeys shortcut' },
  { id: 'keyboard-shortcuts:composer-enter-key', label: 'Enter key', subview: 'keyboard-shortcuts', path: 'Settings → Keyboard Shortcuts → Composer', anchor: 'keyboard-shortcuts:composer-enter-key', keywords: 'enter return key send message new line newline shift enter multi-line multiline prompt composer accidentally sent half-finished' },

  { id: 'system-overview', label: 'Overview', subview: 'system-overview', path: 'Settings → System', keywords: 'connection status workspace path api versions build uptime restart refresh update' },
  { id: 'release-notices', label: 'Release Notices', subview: 'release-notices', path: 'Settings → System', anchor: 'release-notices:list', keywords: 'release notice notices after upgrade to do action needed workspace audit drift got it answered' },
  { id: 'whats-new', label: "What's New", subview: 'whats-new', path: 'Settings → System', keywords: 'changelog release notes version history whats new updates changes released' },
  { id: 'thread-queue', label: 'Thread Queue', subview: 'thread-queue', path: 'Settings → System', keywords: 'thread queue concurrency capacity running queued' },
  { id: 'backup', label: 'Backup', subview: 'backup', path: 'Settings → System' },
  { id: 'memory', label: 'Memory', subview: 'memory', path: 'Settings → System' },
  { id: 'memory:module', label: 'Memory module', subview: 'memory', path: 'Settings → System → Memory', anchor: 'memory:module', keywords: 'memory module classic tree summary trees compactor compaction backfill recall zoom' },
  { id: 'memory:summary-trees', label: 'Summary trees', subview: 'memory', path: 'Settings → System → Memory', anchor: 'memory:summary-trees', keywords: 'summary trees tree memory browser browse workspace threads zoom lines' },
  { id: 'disk-usage', label: 'Disk Usage', subview: 'disk-usage', path: 'Settings → System', keywords: 'storage space disk usage data' },
  { id: 'prompt-footprint', label: 'Workspace Prompt Footprint', subview: 'prompt-footprint', path: 'Settings → System', keywords: 'prompt footprint context size tokens unused apps knowhow ceiling audit' },
  { id: 'environment-variables', label: 'Environment Variables', subview: 'environment-variables', path: 'Settings → System', keywords: 'env var environment variable config' },
  { id: 'debugging', label: 'Debugging', subview: 'debugging', path: 'Settings → System', keywords: 'debug developer diagnostics perf performance instrumentation telemetry lag latency profiling capture context' },

  { id: 'debugging:capture-context', label: 'Capture context per step', subview: 'debugging', path: 'Settings → System → Debugging', anchor: 'debugging:capture-context', keywords: 'capture context step debug llm prompt' },
  { id: 'debugging:perf', label: 'Perf instrumentation', subview: 'debugging', path: 'Settings → System → Debugging', anchor: 'debugging:perf', keywords: 'perf performance instrumentation telemetry lag latency profiling thread open render linkify' },
  { id: 'debugging:animation-speed', label: 'Animation speed', subview: 'debugging', path: 'Settings → System → Debugging', anchor: 'debugging:animation-speed', keywords: 'animation speed transition duration slow motion multiplier' },
  { id: 'debugging:autocorrect', label: 'Autocorrect', subview: 'debugging', path: 'Settings → System → Debugging', anchor: 'debugging:autocorrect', keywords: 'autocorrect auto-correct autocorrection keyboard typing spelling send button submit button tap dead nothing happens iphone ipad', iosOnly: true },
  { id: 'debugging:restart-engine', label: 'Restart engine', subview: 'debugging', path: 'Settings → System → Debugging', anchor: 'debugging:restart-engine', keywords: 'restart engine service launchd relaunch reboot recovery unresponsive stuck', packagedOnly: true },

  { id: 'system:connection', label: 'Connection', subview: 'system-overview', path: 'Settings → System → Overview', anchor: 'system:connection', keywords: 'status workspace path api url' },
  { id: 'system:versions', label: 'Versions', subview: 'system-overview', path: 'Settings → System → Overview', anchor: 'system:versions', keywords: 'lucidos engine client build release uptime' },
  { id: 'system:maintenance', label: 'Maintenance', subview: 'system-overview', path: 'Settings → System → Overview', anchor: 'system:maintenance', keywords: 'restart rebuild refresh update client engine' },
  { id: 'system:update-check', label: 'Check for updates automatically', subview: 'system-overview', path: 'Settings → System → Overview → Maintenance', anchor: 'system:update-check', keywords: 'update check automatic new version release', when: () => releaseCheck.value?.supported === true },
  { id: 'system:update-channel', label: 'Update channel', subview: 'system-overview', path: 'Settings → System → Overview → Maintenance', anchor: 'system:update-channel', keywords: 'update channel stable beta local custom address url latest.json release candidate staged test', when: () => hasUpdateChannelChoice(releaseCheck.value) },
  { id: 'system:installs', label: 'Installs', subview: 'system-overview', path: 'Settings → System → Overview', anchor: 'system:installs', keywords: 'installs install machines desktop app versions' },

  { id: 'whats-new:releases', label: 'Releases', subview: 'whats-new', path: "Settings → System → What's New", anchor: 'whats-new:releases', keywords: 'releases release notes changelog versions' },
  { id: 'debugging:communication-surfaces', label: 'Communication surfaces', subview: 'debugging', path: 'Settings → System → Debugging', anchor: 'debugging:communication-surfaces', keywords: 'communication surfaces toast banner popover dialog samples gallery' },
  { id: 'surfaces:toasts', label: 'Toasts', subview: 'communication-surfaces', path: 'Settings → System → Communication Surfaces', anchor: 'surfaces:toasts', keywords: 'toast toasts samples' },
  { id: 'surfaces:popovers', label: 'Popovers', subview: 'communication-surfaces', path: 'Settings → System → Communication Surfaces', anchor: 'surfaces:popovers', keywords: 'popover popovers samples' },
  { id: 'surfaces:dialogs', label: 'Dialogs', subview: 'communication-surfaces', path: 'Settings → System → Communication Surfaces', anchor: 'surfaces:dialogs', keywords: 'dialog dialogs confirm modal samples' },
  { id: 'surfaces:banners', label: 'Banners', subview: 'communication-surfaces', path: 'Settings → System → Communication Surfaces', anchor: 'surfaces:banners', keywords: 'banner banners samples' },

  { id: 'locale:language', label: 'Language', subview: 'locale', path: 'Settings → Locale', anchor: 'locale:language', keywords: 'language locale respond reply' },
  { id: 'locale:timezone', label: 'Timezone', subview: 'locale', path: 'Settings → Locale', anchor: 'locale:timezone', keywords: 'timezone time zone iana triggers schedule' },

  { id: 'coding-agents:binaries', label: 'Binaries', subview: 'coding-agents', path: 'Settings → Coding Agents', anchor: 'coding-agents:binaries', keywords: 'coding agent claude codex binary path cli override auto-detect' },
  { id: 'coding-agents:permissions', label: 'Permissions', subview: 'coding-agents', path: 'Settings → Coding Agents', anchor: 'coding-agents:permissions', keywords: 'permission mode claude code auto accept edits classifier approve prompt card ask' },
  { id: 'coding-agents:context-ceiling', label: 'Context ceiling', subview: 'coding-agents', path: 'Settings → Coding Agents', anchor: 'coding-agents:context-ceiling', keywords: 'context ceiling hand off handoff fresh session successor size tokens 300k window' },
  { id: 'coding-agents:landing', label: 'Land changes', subview: 'coding-agents', path: 'Settings → Coding Agents', anchor: 'coding-agents:landing', keywords: 'landing lander here another host repository origin' },
  { id: 'coding-agents:main-check', label: 'Main check', subview: 'coding-agents', path: 'Settings → Coding Agents', anchor: 'coding-agents:main-check', keywords: 'main check harden github slow suites here another host off repair red' },
  { id: 'models:cache-keep-alive', label: 'Cache keep-alive', subview: 'models', path: 'Settings → Models', anchor: 'models:cache-keep-alive', keywords: 'cache keep-alive keep warm prompt cache reread rewrite ping beat prefill give up reread cap on off cost claude code lucidos agent' },
  { id: 'models:cache-cards', label: 'Cache cards', subview: 'models', path: 'Settings → Models → Cache keep-alive', anchor: 'models:cache-keep-alive', keywords: 'cache cards warm line kept warm keep-alive prompt cache countdown time left hide show' },
  { id: 'coding-agents:intelligence', label: 'Intelligence ladders', subview: 'coding-agents', path: 'Settings → Coding Agents', anchor: 'coding-agents:intelligence', keywords: 'intelligence ladder claude code codex link' },
  { id: 'intelligence:prices', label: 'Prices', subview: 'intelligence', path: 'Settings → Intelligence', anchor: 'intelligence:prices', keywords: 'price prices price table checked keep prices current trigger daily weekly cost' },
  { id: 'intelligence:ladder-chat', label: 'Intelligence ladder · Lucidos Agent', subview: 'intelligence', path: 'Settings → Intelligence', anchor: 'intelligence:ladder-chat', keywords: 'intelligence ladder rung model effort default picker chat lucidos agent trigger cost time band opus sonnet haiku gpt gemini grok enable disable list customised reset sort order' },
  { id: 'intelligence:ladder-claude-code', label: 'Intelligence ladder · Claude Code', subview: 'intelligence', path: 'Settings → Intelligence', anchor: 'intelligence:ladder-claude-code', keywords: 'intelligence ladder rung model effort default implementer cost time band claude code opus sonnet haiku copy customised reset sort order' },
  { id: 'intelligence:ladder-codex', label: 'Intelligence ladder · Codex', subview: 'intelligence', path: 'Settings → Intelligence', anchor: 'intelligence:ladder-codex', keywords: 'intelligence ladder rung model effort default implementer cost time band codex gpt copy customised reset sort order' },
  { id: 'intelligence:long-prompt-share', label: 'Share of long-prompt calls', subview: 'intelligence', path: 'Settings → Intelligence', anchor: 'intelligence:long-prompt-share', keywords: 'long prompt share 100k tokens slider haiku cost band measured' },
  { id: 'coding-agents:repositories', label: 'Repositories', subview: 'coding-agents', path: 'Settings → Coding Agents', anchor: 'coding-agents:repositories', keywords: 'repository repositories git local clone register external repo' },

  { id: 'access:addresses', label: 'Other addresses', subview: 'access', path: 'Settings → Access', anchor: 'access:addresses', keywords: 'other addresses connect url lan tailnet magicdns address phone open elsewhere' },
  { id: 'access:pair-device', label: 'Pair a device', subview: 'access', path: 'Settings → Access', anchor: 'access:pair-device', keywords: 'pair pairing code qr scan phone new device enrol add' },
  { id: 'access:pair-gateway', label: 'Pair a gateway', subview: 'access', path: 'Settings → Access', anchor: 'access:pair-gateway', keywords: 'pair gateway code another gateway let in unpair relay remote' },
  { id: 'access:remote-gateways', label: 'Remote gateways', subview: 'access', path: 'Settings → Access', anchor: 'access:remote-gateways', keywords: 'remote gateway add another machine cloud server workspaces elsewhere pair' },
  { id: 'access:tailscale', label: 'Tailscale', subview: 'access', path: 'Settings → Access', anchor: 'access:tailscale', keywords: 'tailscale tailnet vpn magicdns serve https sign in' },
  { id: 'access:network', label: 'Network access', subview: 'access', path: 'Settings → Access', anchor: 'access:network', keywords: 'network bind loopback lan address listen expose engine' },
  { id: 'access:external-address', label: 'External address', subview: 'access', path: 'Settings → Access', anchor: 'access:external-address', keywords: 'external address pairing qr vpn wireguard zerotier netbird headscale reverse proxy domain origin' },
  { id: 'access:steps', label: 'Getting Lucidos onto a device', subview: 'access', path: 'Settings → Access', anchor: 'access:steps', keywords: 'install set up setup steps another device this device home screen' },
  { id: 'access:machine', label: 'The machine running Lucidos', subview: 'access', path: 'Settings → Access', anchor: 'access:machine', keywords: 'host machine tailscale tailnet serve https' },
  { id: 'access:device', label: 'This device', subview: 'access', path: 'Settings → Access', anchor: 'access:device', keywords: 'this device tailscale join tailnet phone' },

  { id: 'models:chat', label: 'Chat & triggers', subview: 'models', path: 'Settings → Models', anchor: 'models:chat' },
  { id: 'models:image-generation', label: 'Image generation', subview: 'models', path: 'Settings → Models', anchor: 'models:image-generation' },
  { id: 'models:background-tasks', label: 'Background tasks', subview: 'models', path: 'Settings → Models', anchor: 'models:background-tasks' },
  { id: 'models:chat-model', label: 'Chat model', subview: 'models', path: 'Settings → Models → Chat & triggers', anchor: 'models:chat-model', keywords: 'model reasoning effort thinking tier opus sonnet haiku gpt' },
  { id: 'models:trigger-model', label: 'Trigger model', subview: 'models', path: 'Settings → Models → Chat & triggers', anchor: 'models:trigger-model', keywords: 'trigger model reasoning effort thinking tier opus sonnet haiku gpt scheduled default' },
  { id: 'models:response-style', label: 'Response style', subview: 'models', path: 'Settings → Models', anchor: 'models:response-style', keywords: 'response style brevity verbose verbosity concise minimal short shorter terse length yapping waffle rambling output style tone instruction custom learning teach explain why outcome autopilot' },
  { id: 'models:technical-literacy', label: 'How technical', subview: 'models', path: 'Settings → Models → Response style', anchor: 'models:technical-literacy', keywords: 'technical literacy level jargon plain language simple beginner non-technical developer expert explain vocabulary' },
  { id: 'models:max-tool-calls', label: 'Max tool calls', subview: 'models', path: 'Settings → Models → Chat & triggers', anchor: 'models:max-tool-calls', keywords: 'max tool calls cap limit turn runaway budget' },
  { id: 'models:title-generation', label: 'Title generation', subview: 'models', path: 'Settings → Models → Background tasks', anchor: 'models:title-generation' },
  { id: 'models:image-description', label: 'Image description', subview: 'models', path: 'Settings → Models → Background tasks', anchor: 'models:image-description' },
  { id: 'models:memory-extraction', label: 'Memory extraction', subview: 'models', path: 'Settings → Models → Background tasks', anchor: 'models:memory-extraction' },
  { id: 'models:query-classification', label: 'Query classification', subview: 'models', path: 'Settings → Models → Background tasks', anchor: 'models:query-classification', keywords: 'query classification retrieval memory typesafe jev clef system one judgment' },
  { id: 'models:conversation-summary', label: 'Conversation summary', subview: 'models', path: 'Settings → Models → Background tasks', anchor: 'models:conversation-summary' },
  { id: 'models:read-decision', label: 'Read decision', subview: 'models', path: 'Settings → Models → Background tasks', anchor: 'models:read-decision', keywords: 'read decision review worth reading typesafe jev openai decisions openrouter system one judgment' },
  { id: 'models:command-judge', label: 'Command judge', subview: 'models', path: 'Settings → Models → Background tasks', anchor: 'models:command-judge', keywords: 'command guard judge model safety flash lite luna haiku typesafe jev clef system one judgment backend auto answer coding agent' },
  { id: 'models:wake-decision', label: 'Wake decision', subview: 'models', path: 'Settings → Models → Background tasks', anchor: 'models:wake-decision', keywords: 'wake decision sub-thread child parent filed report typesafe jev openai decisions openrouter system one judgment' },
  { id: 'models:expected-wake-decision', label: 'Expected wake', subview: 'models', path: 'Settings → Models → Background tasks', anchor: 'models:expected-wake-decision', keywords: 'expected wake decision cache keep-alive waiting estimate when the thread wakes typesafe jev openai decisions openrouter system one judgment' },
  { id: 'models:permission-answer', label: 'Permission answer', subview: 'models', path: 'Settings → Models → Background tasks', anchor: 'models:permission-answer', keywords: 'permission answer words voice spoken typed grant allow request judgment typesafe jev openai decisions openrouter system one' },
  { id: 'models:voice', label: 'Voice', subview: 'models', path: 'Settings → Models', anchor: 'models:voice', keywords: 'voice call talk speak speech audio microphone realtime talker transcriber experimental' },
  { id: 'models:voice-enabled', label: 'Voice (experimental)', subview: 'models', path: 'Settings → Models → Voice', anchor: 'models:voice-enabled', keywords: 'voice call enable turn on off home thread', when: voiceReachable },
  { id: 'models:voice-talker', label: 'Talker model', subview: 'models', path: 'Settings → Models → Voice', anchor: 'models:voice-enabled', keywords: 'talker speech to speech realtime voice model', when: voiceReachable },
  { id: 'models:voice-transcriber', label: 'Transcriber model', subview: 'models', path: 'Settings → Models → Voice', anchor: 'models:voice-enabled', keywords: 'transcriber transcription speech to text whisper voice model', when: voiceReachable },
  { id: 'models:voice-talker-voice', label: 'Spoken voice', subview: 'models', path: 'Settings → Models → Voice', anchor: 'models:voice-enabled', keywords: 'spoken voice sounds like speaker persona', when: voiceReachable },
  { id: 'models:voice-resident-sections', label: 'Resident context', subview: 'models', path: 'Settings → Models → Voice', anchor: 'models:voice-enabled', keywords: 'resident context call knows voice sections', when: voiceReachable },
  ...VOICE_RESIDENT_SECTIONS.map((s): SettingsSearchEntry => ({ id: `models:voice-section-${s.id}`, label: s.title, subview: 'models', path: 'Settings → Models → Voice → Resident context', anchor: 'models:voice-enabled', keywords: 'voice resident context call knows', when: voiceReachable })),
  { id: 'models:title-style', label: 'Title style', subview: 'models', path: 'Settings → Models → Background tasks', anchor: 'models:title-style', keywords: 'title style thread name naming kebab case format reset' },
  { id: 'models:change-summary', label: 'Change summary', subview: 'models', path: 'Settings → Models → Background tasks', anchor: 'models:change-summary', keywords: 'change summary pending change diff describe' },
  { id: 'models:summary-compaction', label: 'Summary compaction', subview: 'models', path: 'Settings → Models → Background tasks', anchor: 'models:summary-compaction', keywords: 'summary compaction compactor tree memory', when: () => currentMemoryModule() === 'tree' },
  { id: 'models:response-style-picker', label: 'Style', subview: 'models', path: 'Settings → Models → Response style', anchor: 'models:response-style-picker', keywords: 'response style pick choose preset library' },
  { id: 'models:manage', label: 'Manage models', subview: 'models', path: 'Settings → Models', anchor: 'models:manage', keywords: 'manage models registry add custom model context window remove' },

  { id: providerAnchor('vertex'), label: 'Vertex AI', subview: 'providers', path: PROVIDERS_SETTINGS_PATH, anchor: providerAnchor('vertex'), keywords: 'vertex gcloud gcp google adc region' },
  { id: providerAnchor('opencode-free'), label: 'OpenCode Free (keyless)', subview: 'providers', path: PROVIDERS_SETTINGS_PATH, anchor: providerAnchor('opencode-free'), keywords: 'opencode free keyless no key no account zen relay anonymous trial try' },
  { id: providerAnchor('vertex', 'region'), label: 'Region', subview: 'providers', path: `${PROVIDERS_SETTINGS_PATH} → Vertex AI`, anchor: providerAnchor('vertex') },
  { id: providerAnchor('anthropic'), label: 'Anthropic (direct)', subview: 'providers', path: PROVIDERS_SETTINGS_PATH, anchor: providerAnchor('anthropic'), keywords: 'anthropic claude api key direct' },
  { id: providerAnchor('openai'), label: 'OpenAI (direct)', subview: 'providers', path: PROVIDERS_SETTINGS_PATH, anchor: providerAnchor('openai'), keywords: 'openai gpt api key direct' },
  { id: providerAnchor('openrouter'), label: 'OpenRouter', subview: 'providers', path: PROVIDERS_SETTINGS_PATH, anchor: providerAnchor('openrouter'), keywords: 'openrouter router api key' },
  { id: providerAnchor('xai'), label: 'xAI', subview: 'providers', path: PROVIDERS_SETTINGS_PATH, anchor: providerAnchor('xai'), keywords: 'xai grok api key' },
  { id: providerAnchor('local'), label: 'Local (OpenAI-compatible)', subview: 'providers', path: PROVIDERS_SETTINGS_PATH, anchor: providerAnchor('local'), keywords: 'local ollama lm studio llama vllm openai compatible base url self-hosted' },
  { id: providerAnchor('claude-gateway'), label: 'Claude gateway', subview: 'providers', path: PROVIDERS_SETTINGS_PATH, anchor: providerAnchor('claude-gateway'), keywords: 'claude gateway organisation organization company sign in device code anthropic' },
  { id: providerAnchor('chatgpt-plan'), label: 'ChatGPT plan', subview: 'providers', path: PROVIDERS_SETTINGS_PATH, anchor: providerAnchor('chatgpt-plan'), keywords: 'chatgpt plan subscription plus pro sign in with chatgpt openai gpt usage limit' },
  ...Object.values(SYSTEM_ONE_PROVIDERS).flatMap((p): SettingsSearchEntry[] => [
    ...(p.chatProvider ? [] : [{ id: p.anchor, label: p.label, subview: 'providers', path: PROVIDERS_SETTINGS_PATH, anchor: p.anchor, keywords: 'system one judgment provider api key' } satisfies SettingsSearchEntry]),
    { id: providerAnchor(p.id, 'usage'), label: `${p.label}: in use by`, subview: 'providers', path: `${PROVIDERS_SETTINGS_PATH} → ${p.label}`, anchor: p.anchor, keywords: 'in use by sites command guard query classification read decision judgment system one' },
  ]),

  { id: 'appearance:theme', label: 'Theme', subview: 'appearance', path: 'Settings → Appearance & Behavior', anchor: 'appearance:theme', keywords: 'theme themes look looks colour color scheme palette nord catppuccin gruvbox solarized minimal header focus accent customize' },
  { id: 'appearance:mode', label: 'Mode', subview: 'appearance', path: 'Settings → Appearance & Behavior → Theme', anchor: 'appearance:mode', keywords: 'theme mode dark mode light mode dark light system appearance night' },
  { id: 'appearance:agent-icon-colors', label: 'Agent icon colors', subview: 'appearance', path: 'Settings → Appearance & Behavior → Theme', anchor: 'appearance:agent-icon-colors', keywords: 'agent icon colors colours brand theme muted gray grey marks drawer claude codex lucidos' },
  { id: 'appearance:theme-effects', label: 'Effects', subview: 'appearance', path: 'Settings → Appearance & Behavior → Theme', anchor: 'appearance:theme-effects', keywords: 'glow shadow scanlines effects theme reduce contrast transparency battery sharp text' },
  { id: 'appearance:motion', label: 'Motion', subview: 'appearance', path: 'Settings → Appearance & Behavior', anchor: 'appearance:motion', keywords: 'reduce motion reduced animation animations calm still accessibility vestibular dizzy spinner pulse slide' },
  { id: 'appearance:typography', label: 'Typography', subview: 'appearance', path: 'Settings → Appearance & Behavior', anchor: 'appearance:typography' },
  { id: 'appearance:font', label: 'Font', subview: 'appearance', path: 'Settings → Appearance & Behavior → Typography', anchor: 'appearance:font' },
  { id: 'appearance:ui-scale', label: 'UI scale', subview: 'appearance', path: 'Settings → Appearance & Behavior → Typography', anchor: 'appearance:ui-scale' },
  { id: 'appearance:mobile', label: 'Mobile', subview: 'appearance', path: 'Settings → Appearance & Behavior', anchor: 'appearance:mobile', mobileOnly: true },
  { id: 'appearance:mobile-dynamic-bars', label: 'Dynamic bars', subview: 'appearance', path: 'Settings → Appearance & Behavior → Mobile', anchor: 'appearance:mobile-dynamic-bars', mobileOnly: true, keywords: 'keep header visible hide on scroll prompt sticky pinned' },
  { id: 'appearance:notifications', label: 'Notifications', subview: 'appearance', path: 'Settings → Appearance & Behavior', anchor: 'appearance:notifications', keywords: 'notifications push alerts banners this device' },
  { id: 'appearance:push-notifications', label: 'Push notifications', subview: 'appearance', path: 'Settings → Appearance & Behavior → Notifications', anchor: 'appearance:push-notifications', keywords: 'push notifications enable disable this device alerts banners buzz' },
  { id: 'appearance:in-app-toasts', label: 'In-app toasts', subview: 'appearance', path: 'Settings → Appearance & Behavior → Notifications', anchor: 'appearance:in-app-toasts', keywords: 'toast toasts banner banners popup pop-up in-app notifications disable turn off silence quiet distracting interruption' },
  { id: 'appearance:links', label: 'Links', subview: 'appearance', path: 'Settings → Appearance & Behavior', anchor: 'appearance:links', keywords: 'links open external browser', when: () => externalLinkTargetConfigurable() || isTauri() },
  { id: 'appearance:external-link-target', label: 'Open links in', subview: 'appearance', path: 'Settings → Appearance & Behavior → Links', anchor: 'appearance:external-link-target', keywords: 'external links safari ask share sheet in-app browser open link default browser', iosPwaOnly: true },
  { id: 'appearance:in-app-browser', label: 'Open links in the in-app browser', subview: 'appearance', path: 'Settings → Appearance & Behavior → Links', anchor: 'appearance:in-app-browser', keywords: 'in-app browser pane experimental drawer external link', tauriOnly: true },
  { id: 'appearance:workspace-fonts', label: 'Workspace fonts', subview: 'appearance', path: 'Settings → Appearance & Behavior → Typography', anchor: 'appearance:workspace-fonts', keywords: 'workspace fonts install font custom typeface' },
  { id: 'appearance:style-overrides', label: 'Style overrides', subview: 'appearance', path: 'Settings → Appearance & Behavior', anchor: 'appearance:style-overrides', keywords: 'style remote overrides custom properties tuned reset', when: hasStyleOverrides },
  { id: 'appearance:style-overrides-clear', label: 'Clear style overrides', subview: 'appearance', path: 'Settings → Appearance & Behavior → Style overrides', anchor: 'appearance:style-overrides-clear', keywords: 'clear all reset style remote overrides tuned values', when: hasStyleOverrides },

  { id: 'backup:provider', label: 'Provider', subview: 'backup', path: 'Settings → System → Backup', anchor: 'backup:provider' },
  { id: 'backup:schedule', label: 'Backup schedule', subview: 'backup', path: 'Settings → System → Backup', anchor: 'backup:schedule', keywords: 'daily weekly scheduled automatic backup time' },
  { id: 'backup:retention', label: 'Backups to keep', subview: 'backup', path: 'Settings → System → Backup', anchor: 'backup:retention', keywords: 'retention keep old backups prune' },
  { id: 'backup:key', label: 'Backup key', subview: 'backup', path: 'Settings → System → Backup', anchor: 'backup:key', keywords: 'age encryption recipient verify rotate public key restore' },

  { id: 'accounts:credentials', label: 'Credentials', subview: 'accounts', path: 'Settings → Accounts', anchor: 'accounts:credentials', keywords: 'api key token password secret oauth client app registration' },
  { id: 'accounts:1password', label: '1Password', subview: 'accounts', path: 'Settings → Accounts', anchor: 'accounts:1password', keywords: 'onepassword op service account secret reference vault' },
  { id: 'accounts:connected', label: 'Connected accounts', subview: 'accounts', path: 'Settings → Accounts', anchor: 'accounts:connected', keywords: 'oauth connect sign in google microsoft github dropbox account authorize reconnect disconnect' },

  { id: 'agent-sandbox', label: 'Agent sandbox', subview: 'permissions', path: 'Settings → Permissions', anchor: 'agent-sandbox', keywords: 'sandbox seatbelt landlock secrets isolation agents' },
  { id: 'private-paths', label: 'Private paths', subview: 'permissions', path: 'Settings → Permissions', anchor: 'private-paths', keywords: 'sandbox files folders machine-wide workspace' },
  { id: 'agent-sandbox:unsandboxed', label: 'Sandbox agents', subview: 'permissions', path: 'Settings → Permissions → Agent sandbox', anchor: 'agent-sandbox:unsandboxed', keywords: 'sandbox on enable protection off disable unsandboxed seatbelt landlock agents unavailable' },
  { id: 'command-safety', label: 'Command safety', subview: 'permissions', path: 'Settings → Permissions', anchor: 'command-safety', keywords: 'command guard safety bash python shell judge' },
  { id: 'command-safety:guard', label: 'Command guard', subview: 'permissions', path: 'Settings → Permissions → Command safety', anchor: 'command-safety:guard', keywords: 'command guard safety bash python shell' },
  { id: 'command-safety:judge', label: 'LLM judge', subview: 'permissions', path: 'Settings → Permissions → Command safety', anchor: 'command-safety:judge', keywords: 'command guard llm judge' },
  { id: 'permissions:standing-denies', label: 'Standing denies', subview: 'permissions', path: 'Settings → Permissions', anchor: 'permissions:standing-denies', keywords: 'always deny command rule block refuse denylist never allow bash python' },
  { id: 'permissions:lucidos', label: 'Lucidos Agent permissions', subview: 'permissions', path: 'Settings → Permissions', anchor: 'permissions:lucidos', keywords: 'lucidos agent command allowlist bash python always allow auto allow' },
  { id: 'permissions:coding-agent', label: 'Coding agent permissions', subview: 'permissions', path: 'Settings → Permissions', anchor: 'permissions:coding-agent', keywords: 'claude code permissions codex permissions tool permissions allowed tools allowlist always allow' },
  { id: 'permissions:coding-agent-auto-answer', label: 'Auto-answer coding-agent cards', subview: 'permissions', path: 'Settings → Permissions → Coding agent permissions', anchor: 'permissions:coding-agent-auto-answer', keywords: 'auto answer coding agents codex claude code bash permission card questions auto allow command guard judge' },
  { id: 'permissions:mcp', label: 'MCP tool permissions', subview: 'permissions', path: 'Settings → Permissions', anchor: 'permissions:mcp', keywords: 'mcp model context protocol server tool permissions allowlist always allow' },

  { id: 'mcp:cost', label: 'Context cost', subview: 'mcp', path: 'Settings → MCP Servers', anchor: 'mcp:cost', keywords: 'mcp context cost tokens window per request tool definitions expensive' },
  { id: 'mcp:servers', label: 'Servers', subview: 'mcp', path: 'Settings → MCP Servers', anchor: 'mcp:servers', keywords: 'mcp server start stop remove running auto approve disable tool unusable id dispatch' },
  { id: 'mcp:allowed-tools', label: 'MCP tool permissions', subview: 'mcp', path: 'Settings → MCP Servers', anchor: 'mcp:allowed-tools', keywords: 'mcp allowed tools allowlist always allow permission pattern' },
];

function shortcutSearchEntries(): SettingsSearchEntry[] {
  return SHORTCUT_DEFS.map((def) => ({
    id: `shortcut:${def.id}`,
    label: `${def.label} (${displayBinding(def.id)})`,
    subview: 'keyboard-shortcuts' as Subview,
    path: 'Settings → Keyboard Shortcuts',
    anchor: shortcutSearchAnchor(def.id),
    keywords: `${def.label} ${bindingSearchText(bindingFor(def.id))} keyboard shortcut`,
  }));
}

function allSettingsEntries(): SettingsSearchEntry[] {
  return [...SETTINGS_SEARCH_INDEX, ...shortcutSearchEntries()];
}

export function getSettingsSearchResults(query: string, limit: number): SearchResultItem[] {
  const q = query.trim().toLowerCase();
  const visible = (e: SettingsSearchEntry) =>
    (!e.mobileOnly || isMobile())
    && (!e.packagedOnly || enginePackaged.value)
    && (!e.iosPwaOnly || externalLinkTargetConfigurable())
    && (!e.iosOnly || isIOS())
    && (!e.tauriOnly || isTauri())
    && (!e.gatewayOnly || WORKSPACE_ID !== null)
    && (!e.when || e.when());
  const matches = q
    ? rankByTitle(
        allSettingsEntries().filter(e => visible(e) && `${e.label} ${e.keywords ?? ''}`.toLowerCase().includes(q)),
        q,
        e => e.label,
      )
    : SETTINGS_SEARCH_INDEX.filter(visible);
  return matches.slice(0, limit).map(e => ({
    id: e.id,
    title: e.label,
    subtitle: e.path,
    category: 'settings',
    score: 1.0,
  }));
}

export function findSettingsEntry(id: string): SettingsSearchEntry | undefined {
  const current = aliasRetiredSettingsAnchor(id);
  return allSettingsEntries().find(e => e.id === current);
}

export function settingsLinkAnchor(view: string, fragment: string): string {
  const current = aliasRetiredSettingsAnchor(fragment);
  const entries = allSettingsEntries().filter(e => e.subview === view && e.anchor !== undefined);
  const entry = entries.find(e => e.id === current || e.anchor === current)
    ?? entries.find(e => e.id.slice(e.id.indexOf(':') + 1) === current);
  return entry?.anchor ?? (fragment.includes(':') ? current : `${view}:${current}`);
}

export function settingsSearchCovers(anchor: string): boolean {
  return allSettingsEntries().some(e => e.anchor === anchor || e.id === anchor);
}

export function settingsSearchEntryIds(): string[] {
  return SETTINGS_SEARCH_INDEX.map(e => e.id);
}
