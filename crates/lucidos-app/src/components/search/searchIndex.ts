import { enginePackaged, releaseCheck, aliasRetiredSettingsAnchor, type SettingsSubview } from '../../store/store';
import type { SearchResultItem } from '../../api/client';
import { SHORTCUT_DEFS, bindingSearchText, shortcutSearchAnchor } from '../../utils/shortcuts';
import { displayBinding, bindingFor } from '../../store/actions/keybindings';
import { isMobile } from '../../utils/viewport';
import { isIOS, isTauri } from '../../utils/platform';
import { WORKSPACE_ID } from '../../utils/basePath';
// The one definition of "this client can actually act on the external-link
// target", shared with the Settings row and its nav entry so search can never
// offer a result that lands on nothing.
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
  /** Unique result id; also used as the recents key. */
  id: string;
  /** Label as it appears in the UI — single source of truth (no Rust ↔ UI drift). */
  label: string;
  /** Subview to switch to when selected. */
  subview: Subview;
  /** Breadcrumb path shown as the result subtitle. */
  path: string;
  /** Optional `data-search-anchor` value to scroll/highlight after navigation. */
  anchor?: string;
  /** Extra free-text matched in addition to the label (e.g. key-combo aliases
   *  like "ctrl k" for a shortcut). Not shown in the UI. */
  keywords?: string;
  /** Only surfaced in search results on a mobile-width viewport — the matching
   *  settings row is itself hidden on desktop (e.g. the Mobile section), so a
   *  desktop result would navigate to a row that doesn't render. */
  mobileOnly?: boolean;
  /** Only surfaced on a PACKAGED install (the `packaged` flag from /health), for
   *  the same reason as `mobileOnly`: the matching row renders only there (e.g.
   *  Debugging's Restart engine, whose dev counterpart is Overview's
   *  Rebuild & Restart), so a dev result would land on nothing. */
  packagedOnly?: boolean;
  /** Only surfaced on an INSTALLED iOS PWA, for the same reason as the two
   *  above. Strictly narrower than `mobileOnly`: a narrow desktop window and
   *  mobile Chrome both pass `isMobile()` but are not standalone iOS, and the
   *  row behind this flag (Appearance & Behavior → Links → Open links in)
   *  renders only there. */
  iosPwaOnly?: boolean;
  /** Only surfaced on iPhone and iPad, in a Safari tab and the installed PWA
   *  alike. Wider than `iosPwaOnly`, and for the same reason: the Autocorrect
   *  row (Debugging) renders only on iOS, where its bug lives. */
  iosOnly?: boolean;
  /** Only surfaced under Tauri, for the same reason again: the in-app browser
   *  opens a desktop webview, so its toggle renders nowhere else. */
  tauriOnly?: boolean;
  /** Only surfaced on a page the workspace gateway served, once more for the
   *  same reason: Paired devices reads the gateway's own auth surface and
   *  renders nothing when there is no gateway to ask. A page on a direct
   *  engine port resolves that path against the engine and gets a 404. */
  gatewayOnly?: boolean;
  /** Only surfaced while this holds, for a row whose render condition no flag
   *  above names. Pass the same predicate the row renders on. */
  when?: () => boolean;
}

/**
 * Searchable Settings entries. Top-level entries (no anchor) just open the subview.
 * Nested entries scroll to a `[data-search-anchor]` element and pulse it.
 *
 * Labels match the UI. `settings-anchors-searchable.test.ts` fails on a
 * rendered search anchor that no entry covers.
 */
const hasStyleOverrides = () => Object.keys(currentStyleOverrides()).length > 0;

const SETTINGS_SEARCH_INDEX: SettingsSearchEntry[] = [
  // Top-level subviews (one per SETTINGS_NAV_ITEMS entry)
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
  { id: 'devices', label: 'Devices', subview: 'devices', path: 'Settings', keywords: 'device phone laptop push notifications rename remove last seen' },
  { id: 'users', label: 'Users', subview: 'users', path: 'Settings', keywords: 'people person operator team member teammate user email add remove offboard whose device', gatewayOnly: true },
  // Pairing lives on the same row as push now, so Revoke is found on the
  // Devices page rather than under Access. Still gateway-gated: with none, no
  // row carries a Revoke button and the word would land on nothing.
  { id: 'devices:paired', label: 'Paired devices', subview: 'devices', path: 'Settings → Devices', anchor: 'devices:list', keywords: 'paired pairing device revoke unpair sign out cut off network access', gatewayOnly: true },
  { id: 'system', label: 'System', subview: 'system', path: 'Settings' },
  { id: 'appearance', label: 'Appearance & Behavior', subview: 'appearance', path: 'Settings', keywords: 'appearance interface behavior theme font scale links browser' },
  { id: 'keyboard-shortcuts', label: 'Keyboard Shortcuts', subview: 'keyboard-shortcuts', path: 'Settings', keywords: 'keybindings hotkeys shortcut' },
  { id: 'keyboard-shortcuts:composer-enter-key', label: 'Enter key', subview: 'keyboard-shortcuts', path: 'Settings → Keyboard Shortcuts → Composer', anchor: 'keyboard-shortcuts:composer-enter-key', keywords: 'enter return key send message new line newline shift enter multi-line multiline prompt composer accidentally sent half-finished' },

  // System subpanels
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

  // System → Debugging rows
  { id: 'debugging:capture-context', label: 'Capture context per step', subview: 'debugging', path: 'Settings → System → Debugging', anchor: 'debugging:capture-context', keywords: 'capture context step debug llm prompt' },
  { id: 'debugging:perf', label: 'Perf instrumentation', subview: 'debugging', path: 'Settings → System → Debugging', anchor: 'debugging:perf', keywords: 'perf performance instrumentation telemetry lag latency profiling thread open render linkify' },
  { id: 'debugging:animation-speed', label: 'Animation speed', subview: 'debugging', path: 'Settings → System → Debugging', anchor: 'debugging:animation-speed', keywords: 'animation speed transition duration slow motion multiplier' },
  { id: 'debugging:autocorrect', label: 'Autocorrect', subview: 'debugging', path: 'Settings → System → Debugging', anchor: 'debugging:autocorrect', keywords: 'autocorrect auto-correct autocorrection keyboard typing spelling send button submit button tap dead nothing happens iphone ipad', iosOnly: true },
  { id: 'debugging:restart-engine', label: 'Restart engine', subview: 'debugging', path: 'Settings → System → Debugging', anchor: 'debugging:restart-engine', keywords: 'restart engine service launchd relaunch reboot recovery unresponsive stuck', packagedOnly: true },

  // System → Overview rows
  { id: 'system:connection', label: 'Connection', subview: 'system-overview', path: 'Settings → System → Overview', anchor: 'system:connection', keywords: 'status workspace path api url' },
  { id: 'system:versions', label: 'Versions', subview: 'system-overview', path: 'Settings → System → Overview', anchor: 'system:versions', keywords: 'lucidos engine client build release uptime' },
  { id: 'system:maintenance', label: 'Maintenance', subview: 'system-overview', path: 'Settings → System → Overview', anchor: 'system:maintenance', keywords: 'restart rebuild refresh update client engine' },
  { id: 'system:update-check', label: 'Check for updates automatically', subview: 'system-overview', path: 'Settings → System → Overview → Maintenance', anchor: 'system:update-check', keywords: 'update check automatic new version release', when: () => releaseCheck.value?.supported === true },
  { id: 'system:update-address', label: 'Update address', subview: 'system-overview', path: 'Settings → System → Overview → Maintenance', anchor: 'system:update-address', keywords: 'update address url latest.json release candidate rc staged test override', when: () => releaseCheck.value?.update_address !== undefined },
  { id: 'system:installs', label: 'Installs', subview: 'system-overview', path: 'Settings → System → Overview', anchor: 'system:installs', keywords: 'installs install machines desktop app versions' },

  // What's New, and the Communication Surfaces sample page Debugging opens
  { id: 'whats-new:releases', label: 'Releases', subview: 'whats-new', path: "Settings → System → What's New", anchor: 'whats-new:releases', keywords: 'releases release notes changelog versions' },
  { id: 'debugging:communication-surfaces', label: 'Communication surfaces', subview: 'debugging', path: 'Settings → System → Debugging', anchor: 'debugging:communication-surfaces', keywords: 'communication surfaces toast banner popover dialog samples gallery' },
  { id: 'surfaces:toasts', label: 'Toasts', subview: 'communication-surfaces', path: 'Settings → System → Communication Surfaces', anchor: 'surfaces:toasts', keywords: 'toast toasts samples' },
  { id: 'surfaces:popovers', label: 'Popovers', subview: 'communication-surfaces', path: 'Settings → System → Communication Surfaces', anchor: 'surfaces:popovers', keywords: 'popover popovers samples' },
  { id: 'surfaces:dialogs', label: 'Dialogs', subview: 'communication-surfaces', path: 'Settings → System → Communication Surfaces', anchor: 'surfaces:dialogs', keywords: 'dialog dialogs confirm modal samples' },
  { id: 'surfaces:banners', label: 'Banners', subview: 'communication-surfaces', path: 'Settings → System → Communication Surfaces', anchor: 'surfaces:banners', keywords: 'banner banners samples' },

  // Locale subview
  { id: 'locale:language', label: 'Language', subview: 'locale', path: 'Settings → Locale', anchor: 'locale:language', keywords: 'language locale respond reply' },
  { id: 'locale:timezone', label: 'Timezone', subview: 'locale', path: 'Settings → Locale', anchor: 'locale:timezone', keywords: 'timezone time zone iana triggers schedule' },

  // Coding Agents subview
  { id: 'coding-agents:binaries', label: 'Binaries', subview: 'coding-agents', path: 'Settings → Coding Agents', anchor: 'coding-agents:binaries', keywords: 'coding agent claude codex binary path cli override auto-detect' },
  { id: 'coding-agents:permissions', label: 'Permissions', subview: 'coding-agents', path: 'Settings → Coding Agents', anchor: 'coding-agents:permissions', keywords: 'permission mode claude code auto accept edits classifier approve prompt card ask' },
  { id: 'coding-agents:context-ceiling', label: 'Context ceiling', subview: 'coding-agents', path: 'Settings → Coding Agents', anchor: 'coding-agents:context-ceiling', keywords: 'context ceiling hand off handoff fresh session successor size tokens 300k window' },
  { id: 'models:cache-keep-alive', label: 'Cache keep-alive', subview: 'models', path: 'Settings → Models', anchor: 'models:cache-keep-alive', keywords: 'cache keep-alive keep warm prompt cache reread rewrite ping beat prefill give up reread cap on off cost claude code lucidos agent' },
  // Like Region below: the Cache cards row renders only while the keep-alive
  // is on, so this lands on the section title that carries its switch.
  { id: 'models:cache-cards', label: 'Cache cards', subview: 'models', path: 'Settings → Models → Cache keep-alive', anchor: 'models:cache-keep-alive', keywords: 'cache cards warm line kept warm keep-alive prompt cache countdown time left hide show' },
  { id: 'coding-agents:intelligence', label: 'Intelligence ladders', subview: 'coding-agents', path: 'Settings → Coding Agents', anchor: 'coding-agents:intelligence', keywords: 'intelligence ladder claude code codex link' },
  { id: 'intelligence:prices', label: 'Prices', subview: 'intelligence', path: 'Settings → Intelligence', anchor: 'intelligence:prices', keywords: 'price prices price table checked keep prices current trigger daily weekly cost' },
  { id: 'intelligence:ladder-chat', label: 'Intelligence ladder · Lucidos Agent', subview: 'intelligence', path: 'Settings → Intelligence', anchor: 'intelligence:ladder-chat', keywords: 'intelligence ladder rung model effort default picker chat lucidos agent trigger cost time band opus sonnet haiku gpt gemini grok enable disable list customised reset sort order' },
  { id: 'intelligence:ladder-claude-code', label: 'Intelligence ladder · Claude Code', subview: 'intelligence', path: 'Settings → Intelligence', anchor: 'intelligence:ladder-claude-code', keywords: 'intelligence ladder rung model effort default implementer cost time band claude code opus sonnet haiku copy customised reset sort order' },
  { id: 'intelligence:ladder-codex', label: 'Intelligence ladder · Codex', subview: 'intelligence', path: 'Settings → Intelligence', anchor: 'intelligence:ladder-codex', keywords: 'intelligence ladder rung model effort default implementer cost time band codex gpt copy customised reset sort order' },
  { id: 'intelligence:long-prompt-share', label: 'Share of long-prompt calls', subview: 'intelligence', path: 'Settings → Intelligence', anchor: 'intelligence:long-prompt-share', keywords: 'long prompt share 100k tokens slider haiku cost band measured' },
  { id: 'coding-agents:repositories', label: 'Repositories', subview: 'coding-agents', path: 'Settings → Coding Agents', anchor: 'coding-agents:repositories', keywords: 'repository repositories git local clone register external repo' },

  // Access subview
  // Ungated on purpose: the section renders in any browser now, deriving its
  // tailnet rows from two plain-HTTP reads. Gating it hid the address the
  // browser user came looking for, from behind the section showing it.
  { id: 'access:addresses', label: 'Other addresses', subview: 'access', path: 'Settings → Access', anchor: 'access:addresses', keywords: 'other addresses connect url lan tailnet magicdns address phone open elsewhere' },
  { id: 'access:pair-device', label: 'Pair a device', subview: 'access', path: 'Settings → Access', anchor: 'access:pair-device', keywords: 'pair pairing code qr scan phone new device enrol add' },
  { id: 'access:pair-gateway', label: 'Pair a gateway', subview: 'access', path: 'Settings → Access', anchor: 'access:pair-gateway', keywords: 'pair gateway code another gateway let in unpair relay remote' },
  { id: 'access:remote-gateways', label: 'Remote gateways', subview: 'access', path: 'Settings → Access', anchor: 'access:remote-gateways', keywords: 'remote gateway add another machine cloud server workspaces elsewhere pair' },
  { id: 'access:tailscale', label: 'Tailscale', subview: 'access', path: 'Settings → Access', anchor: 'access:tailscale', keywords: 'tailscale tailnet vpn magicdns serve https sign in' },
  { id: 'access:network', label: 'Network access', subview: 'access', path: 'Settings → Access', anchor: 'access:network', keywords: 'network bind loopback lan address listen expose engine' },
  { id: 'access:external-address', label: 'External address', subview: 'access', path: 'Settings → Access', anchor: 'access:external-address', keywords: 'external address pairing qr vpn wireguard zerotier netbird headscale reverse proxy domain origin' },
  // Its heading changes with the device's state, so the label names the section.
  { id: 'access:steps', label: 'Getting Lucidos onto a device', subview: 'access', path: 'Settings → Access', anchor: 'access:steps', keywords: 'install set up setup steps another device this device home screen' },
  { id: 'access:machine', label: 'The machine running Lucidos', subview: 'access', path: 'Settings → Access', anchor: 'access:machine', keywords: 'host machine tailscale tailnet serve https' },
  { id: 'access:device', label: 'This device', subview: 'access', path: 'Settings → Access', anchor: 'access:device', keywords: 'this device tailscale join tailnet phone' },

  // Models subview
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
  // The rows below render only while voice is on. So they land on the
  // switch row. A relayed workspace draws no switch, so there only
  // the Voice heading above is found.
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

  // Providers subview
  { id: providerAnchor('vertex'), label: 'Vertex AI', subview: 'providers', path: PROVIDERS_SETTINGS_PATH, anchor: providerAnchor('vertex'), keywords: 'vertex gcloud gcp google adc region' },
  // Its own row, not just a keyword on the page entry. It is the one provider a
  // user with no key can turn on. So "free" lands on the switch, rather than on
  // the top of a page they then have to scan.
  { id: providerAnchor('opencode-free'), label: 'OpenCode Free (keyless)', subview: 'providers', path: PROVIDERS_SETTINGS_PATH, anchor: providerAnchor('opencode-free'), keywords: 'opencode free keyless no key no account zen relay anonymous trial try' },
  // Lands on the Vertex header, not on the Region row itself. That row sits
  // inside the provider's block, which renders only while Vertex is switched
  // on. An anchor pointing at it scrolls to nothing whenever it is off. The
  // header is always there, and carries the switch that brings the row back.
  { id: providerAnchor('vertex', 'region'), label: 'Region', subview: 'providers', path: `${PROVIDERS_SETTINGS_PATH} → Vertex AI`, anchor: providerAnchor('vertex') },
  { id: providerAnchor('anthropic'), label: 'Anthropic (direct)', subview: 'providers', path: PROVIDERS_SETTINGS_PATH, anchor: providerAnchor('anthropic'), keywords: 'anthropic claude api key direct' },
  { id: providerAnchor('openai'), label: 'OpenAI (direct)', subview: 'providers', path: PROVIDERS_SETTINGS_PATH, anchor: providerAnchor('openai'), keywords: 'openai gpt api key direct' },
  { id: providerAnchor('openrouter'), label: 'OpenRouter', subview: 'providers', path: PROVIDERS_SETTINGS_PATH, anchor: providerAnchor('openrouter'), keywords: 'openrouter router api key' },
  { id: providerAnchor('xai'), label: 'xAI', subview: 'providers', path: PROVIDERS_SETTINGS_PATH, anchor: providerAnchor('xai'), keywords: 'xai grok api key' },
  { id: providerAnchor('local'), label: 'Local (OpenAI-compatible)', subview: 'providers', path: PROVIDERS_SETTINGS_PATH, anchor: providerAnchor('local'), keywords: 'local ollama lm studio llama vllm openai compatible base url self-hosted' },
  { id: providerAnchor('claude-gateway'), label: 'Claude gateway', subview: 'providers', path: PROVIDERS_SETTINGS_PATH, anchor: providerAnchor('claude-gateway'), keywords: 'claude gateway organisation organization company sign in device code anthropic' },
  { id: providerAnchor('chatgpt-plan'), label: 'ChatGPT plan', subview: 'providers', path: PROVIDERS_SETTINGS_PATH, anchor: providerAnchor('chatgpt-plan'), keywords: 'chatgpt plan subscription plus pro sign in with chatgpt openai gpt usage limit' },
  // A provider's "In use by" row sits in its folded body, so it lands on the
  // provider's header, which always renders. A chat provider's header has its
  // own entry above.
  ...Object.values(SYSTEM_ONE_PROVIDERS).flatMap((p): SettingsSearchEntry[] => [
    ...(p.chatProvider ? [] : [{ id: p.anchor, label: p.label, subview: 'providers', path: PROVIDERS_SETTINGS_PATH, anchor: p.anchor, keywords: 'system one judgment provider api key' } satisfies SettingsSearchEntry]),
    { id: providerAnchor(p.id, 'usage'), label: `${p.label}: in use by`, subview: 'providers', path: `${PROVIDERS_SETTINGS_PATH} → ${p.label}`, anchor: p.anchor, keywords: 'in use by sites command guard query classification read decision judgment system one' },
  ]),

  // Appearance & Behavior subview (Links absorbed the retired Links and
  // Experimental categories, so its two rows keep their own platform flags)
  { id: 'appearance:theme', label: 'Theme', subview: 'appearance', path: 'Settings → Appearance & Behavior', anchor: 'appearance:theme', keywords: 'theme themes look looks colour color scheme palette nord catppuccin gruvbox solarized minimal header focus accent customize' },
  { id: 'appearance:mode', label: 'Mode', subview: 'appearance', path: 'Settings → Appearance & Behavior → Theme', anchor: 'appearance:mode', keywords: 'theme mode dark mode light mode dark light system appearance night' },
  { id: 'appearance:agent-brand-colours', label: 'Agent brand colours', subview: 'appearance', path: 'Settings → Appearance & Behavior → Theme', anchor: 'appearance:agent-brand-colours', keywords: 'agent brand colours colors icons marks muted claude codex lucidos' },
  { id: 'appearance:theme-effects', label: 'Effects', subview: 'appearance', path: 'Settings → Appearance & Behavior → Theme', anchor: 'appearance:theme-effects', keywords: 'glow shadow scanlines effects theme reduce contrast transparency battery sharp text' },
  { id: 'appearance:motion', label: 'Motion', subview: 'appearance', path: 'Settings → Appearance & Behavior', anchor: 'appearance:motion', keywords: 'reduce motion reduced animation animations calm still accessibility vestibular dizzy spinner pulse slide' },
  { id: 'appearance:typography', label: 'Typography', subview: 'appearance', path: 'Settings → Appearance & Behavior', anchor: 'appearance:typography' },
  { id: 'appearance:font', label: 'Font', subview: 'appearance', path: 'Settings → Appearance & Behavior → Typography', anchor: 'appearance:font' },
  { id: 'appearance:ui-scale', label: 'UI scale', subview: 'appearance', path: 'Settings → Appearance & Behavior → Typography', anchor: 'appearance:ui-scale' },
  { id: 'appearance:mobile', label: 'Mobile', subview: 'appearance', path: 'Settings → Appearance & Behavior', anchor: 'appearance:mobile', mobileOnly: true },
  { id: 'appearance:mobile-dynamic-bars', label: 'Dynamic bars', subview: 'appearance', path: 'Settings → Appearance & Behavior → Mobile', anchor: 'appearance:mobile-dynamic-bars', mobileOnly: true, keywords: 'keep header visible hide on scroll prompt sticky pinned' },
  // The current device's push switch, the same one its row in Devices carries.
  // Both entries are kept: someone hunting "notifications" means the device they
  // are holding, someone hunting "devices" means the fleet.
  { id: 'appearance:notifications', label: 'Notifications', subview: 'appearance', path: 'Settings → Appearance & Behavior', anchor: 'appearance:notifications', keywords: 'notifications push alerts banners this device' },
  { id: 'appearance:push-notifications', label: 'Push notifications', subview: 'appearance', path: 'Settings → Appearance & Behavior → Notifications', anchor: 'appearance:push-notifications', keywords: 'push notifications enable disable this device alerts banners buzz' },
  // "banner" and "popup" are what people call a toast when they want it gone.
  // Neither is the canonical word, and both have to find this row.
  { id: 'appearance:in-app-toasts', label: 'In-app toasts', subview: 'appearance', path: 'Settings → Appearance & Behavior → Notifications', anchor: 'appearance:in-app-toasts', keywords: 'toast toasts banner banners popup pop-up in-app notifications disable turn off silence quiet distracting interruption' },
  // The Links section renders when either of its two rows does.
  { id: 'appearance:links', label: 'Links', subview: 'appearance', path: 'Settings → Appearance & Behavior', anchor: 'appearance:links', keywords: 'links open external browser', when: () => externalLinkTargetConfigurable() || isTauri() },
  { id: 'appearance:external-link-target', label: 'Open links in', subview: 'appearance', path: 'Settings → Appearance & Behavior → Links', anchor: 'appearance:external-link-target', keywords: 'external links safari ask share sheet in-app browser open link default browser', iosPwaOnly: true },
  { id: 'appearance:in-app-browser', label: 'Open links in the in-app browser', subview: 'appearance', path: 'Settings → Appearance & Behavior → Links', anchor: 'appearance:in-app-browser', keywords: 'in-app browser pane experimental drawer external link', tauriOnly: true },
  { id: 'appearance:workspace-fonts', label: 'Workspace fonts', subview: 'appearance', path: 'Settings → Appearance & Behavior → Typography', anchor: 'appearance:workspace-fonts', keywords: 'workspace fonts install font custom typeface' },
  // Rendered only while the live style remote has tuned something.
  { id: 'appearance:style-overrides', label: 'Style overrides', subview: 'appearance', path: 'Settings → Appearance & Behavior', anchor: 'appearance:style-overrides', keywords: 'style remote overrides custom properties tuned reset', when: hasStyleOverrides },
  { id: 'appearance:style-overrides-clear', label: 'Clear style overrides', subview: 'appearance', path: 'Settings → Appearance & Behavior → Style overrides', anchor: 'appearance:style-overrides-clear', keywords: 'clear all reset style remote overrides tuned values', when: hasStyleOverrides },

  // Backup subview (restore moved to the workspace picker — no in-app entry)
  { id: 'backup:provider', label: 'Provider', subview: 'backup', path: 'Settings → System → Backup', anchor: 'backup:provider' },
  { id: 'backup:schedule', label: 'Backup schedule', subview: 'backup', path: 'Settings → System → Backup', anchor: 'backup:schedule', keywords: 'daily weekly scheduled automatic backup time' },
  { id: 'backup:retention', label: 'Backups to keep', subview: 'backup', path: 'Settings → System → Backup', anchor: 'backup:retention', keywords: 'retention keep old backups prune' },
  { id: 'backup:key', label: 'Backup key', subview: 'backup', path: 'Settings → System → Backup', anchor: 'backup:key', keywords: 'age encryption recipient verify rotate public key restore' },

  // Accounts subview
  { id: 'accounts:credentials', label: 'Credentials', subview: 'accounts', path: 'Settings → Accounts', anchor: 'accounts:credentials', keywords: 'api key token password secret oauth client app registration' },
  { id: 'accounts:1password', label: '1Password', subview: 'accounts', path: 'Settings → Accounts', anchor: 'accounts:1password', keywords: 'onepassword op service account secret reference vault' },
  // Renamed from "OAuth", which named the protocol rather than the thing. The
  // old word stays searchable via keywords so nobody loses the entry.
  { id: 'accounts:connected', label: 'Connected accounts', subview: 'accounts', path: 'Settings → Accounts', anchor: 'accounts:connected', keywords: 'oauth connect sign in google microsoft github dropbox account authorize reconnect disconnect' },

  // Permissions subview (Agent sandbox, Command safety, the two allowlist editors)
  { id: 'agent-sandbox', label: 'Agent sandbox', subview: 'permissions', path: 'Settings → Permissions', anchor: 'agent-sandbox', keywords: 'sandbox seatbelt landlock secrets isolation agents' },
  { id: 'agent-sandbox:unsandboxed', label: 'Sandbox agents', subview: 'permissions', path: 'Settings → Permissions → Agent sandbox', anchor: 'agent-sandbox:unsandboxed', keywords: 'sandbox on enable protection off disable unsandboxed seatbelt landlock agents unavailable' },
  { id: 'command-safety', label: 'Command safety', subview: 'permissions', path: 'Settings → Permissions', anchor: 'command-safety', keywords: 'command guard safety bash python shell judge' },
  { id: 'command-safety:guard', label: 'Command guard', subview: 'permissions', path: 'Settings → Permissions → Command safety', anchor: 'command-safety:guard', keywords: 'command guard safety bash python shell' },
  { id: 'command-safety:judge', label: 'LLM judge', subview: 'permissions', path: 'Settings → Permissions → Command safety', anchor: 'command-safety:judge', keywords: 'command guard llm judge' },
  { id: 'permissions:lucidos', label: 'Lucidos Agent permissions', subview: 'permissions', path: 'Settings → Permissions', anchor: 'permissions:lucidos', keywords: 'lucidos agent command allowlist bash python always allow auto allow' },
  { id: 'permissions:coding-agent', label: 'Coding agent permissions', subview: 'permissions', path: 'Settings → Permissions', anchor: 'permissions:coding-agent', keywords: 'claude code permissions codex permissions tool permissions allowed tools allowlist always allow' },
  { id: 'permissions:coding-agent-auto-answer', label: 'Auto-answer coding-agent cards', subview: 'permissions', path: 'Settings → Permissions → Coding agent permissions', anchor: 'permissions:coding-agent-auto-answer', keywords: 'auto answer coding agents codex claude code bash permission card questions auto allow command guard judge' },
  { id: 'permissions:mcp', label: 'MCP tool permissions', subview: 'permissions', path: 'Settings → Permissions', anchor: 'permissions:mcp', keywords: 'mcp model context protocol server tool permissions allowlist always allow' },

  // MCP Servers subview
  { id: 'mcp:cost', label: 'Context cost', subview: 'mcp', path: 'Settings → MCP Servers', anchor: 'mcp:cost', keywords: 'mcp context cost tokens window per request tool definitions expensive' },
  { id: 'mcp:servers', label: 'Servers', subview: 'mcp', path: 'Settings → MCP Servers', anchor: 'mcp:servers', keywords: 'mcp server start stop remove running auto approve disable tool unusable id dispatch' },
  { id: 'mcp:allowed-tools', label: 'MCP tool permissions', subview: 'mcp', path: 'Settings → MCP Servers', anchor: 'mcp:allowed-tools', keywords: 'mcp allowed tools allowlist always allow permission pattern' },
];

/** Per-shortcut search entries, synthesized from the registry so they reflect
 *  the user's CURRENT (possibly-customized) binding. Each carries key-combo
 *  aliases ("ctrl k", "ctrl+k", "cmd k", …) as keywords so typing a combo finds
 *  it; selecting one lands on that shortcut's row in Keyboard Shortcuts. */
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

/** Filter the index by query (case-insensitive substring over label + keywords),
 *  rank by label best first, and return as SearchResultItems. Ranked before the
 *  cut, so a strong label late in the index survives it. An empty query lists
 *  the static settings index only (not every shortcut), in index order. */
export function getSettingsSearchResults(query: string, limit: number): SearchResultItem[] {
  const q = query.trim().toLowerCase();
  // Mobile-only rows are hidden in Settings on desktop, so don't surface them as
  // search results there — selecting one would land on a row that doesn't render.
  // Packaged-only rows are gated the same way, against the /health `packaged` flag.
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

/** Resolves a retired id too: Search recents are persisted verbatim. */
export function findSettingsEntry(id: string): SettingsSearchEntry | undefined {
  const current = aliasRetiredSettingsAnchor(id);
  return allSettingsEntries().find(e => e.id === current);
}

/** The anchor a `settings:<view>#<fragment>` link lands on. The fragment is a
 *  row's search id after its colon, so a page whose anchors carry another
 *  prefix still resolves. A row inside a fold lands on the ancestor its entry
 *  names. A fragment the index lacks stays `<view>:<fragment>`. */
export function settingsLinkAnchor(view: string, fragment: string): string {
  const entry = allSettingsEntries().find(e =>
    e.subview === view && e.anchor !== undefined && e.id.slice(e.id.indexOf(':') + 1) === fragment);
  return entry?.anchor ?? `${view}:${fragment}`;
}

/** Whether some entry lands on `anchor`, or takes it as its id and lands on an
 *  ancestor that always renders. */
export function settingsSearchCovers(anchor: string): boolean {
  return allSettingsEntries().some(e => e.anchor === anchor || e.id === anchor);
}

/** Ids of the STATIC index. The settings-nav guard walks them to check each
 *  subview is live and each anchor renders, without this module exporting the
 *  whole array. `shortcut-search-landing.test.tsx` checks the synthesized
 *  per-shortcut entries instead. */
export function settingsSearchEntryIds(): string[] {
  return SETTINGS_SEARCH_INDEX.map(e => e.id);
}
