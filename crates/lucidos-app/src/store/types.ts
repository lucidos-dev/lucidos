import type { BackgroundWaitTiming, CacheKeepAliveStatus, CeilingCrossing, FreshSessionReason, HandoffOpening, TileSize, WidgetSize } from '../generated/thread-event-wire';
import { errorDetail } from '../utils/errorDetail';
import type { PluginLocalChangesResult } from '../api/client/apps';
import type { WebhookIngress, WebhookRefusals } from '../api/client/webhooks';
import type {
  AnswerKind,
  EventSubscription,
  EventWaitCancelCause,
  FormRequestEvent,
  FormRequestOutcome,
  QuestionOption,
} from './thread-events/thread-event-types';
import type {
  ApiUsage,
  ContextProducer,
  ContextSection,
  MessageOrigin,
  PluginSetupOccasion,
  RecalledMemory,
  SideEffectCategory,
} from '../generated/thread-event-wire';
import type { Tap } from '@lucidos/sdk';

export type {
  ApiUsage,
  ContextProducer,
  ContextRole,
  ContextSection,
  MemorySource,
  FireRetry,
  RetryOf,
  RetrySkipReason,
  ModalityUsage,
  RecalledMemory,
  SideEffectCategory,
} from '../generated/thread-event-wire';

export type Loadable<T> =
  | { status: 'not-loaded' }
  | { status: 'loading' }
  | { status: 'loaded'; data: T }
  | { status: 'failed'; error: string; httpCode?: number };

export function loadedOr<T>(loadable: Loadable<T>, fallback: T): T {
  return loadable.status === 'loaded' ? loadable.data : fallback;
}

export function toFailed<T>(error: unknown): Loadable<T> {
  if (error instanceof Error && 'httpCode' in error && 'reason' in error) {
    return { status: 'failed', error: (error as { reason: string }).reason, httpCode: (error as { httpCode: number }).httpCode };
  }
  return { status: 'failed', error: errorDetail(error) };
}

export function setLoadingIfFresh<T>(signal: { value: Loadable<T> }): void {
  signal.value = loadingIfFresh(signal.value);
}

export function loadingIfFresh<T>(prev: Loadable<T>): Loadable<T> {
  return prev.status === 'loaded' ? prev : { status: 'loading' };
}

export function failedIfFresh<T>(prev: Loadable<T>, error: unknown): Loadable<T> {
  return prev.status === 'loaded' ? prev : toFailed(error);
}


export interface IngressReading {
  ingress: WebhookIngress;
  receivedAt: number;
}

export interface RefusalReading {
  refusals: WebhookRefusals;
  receivedAt: number;
}

export const MENU_ITEMS = ['files', 'apps', 'plugins', 'triggers', 'settings', 'changes', 'notifications'] as const;
export type MenuItem = typeof MENU_ITEMS[number];

export const MENU_ITEM_LABELS: Record<MenuItem, string> = {
  files: 'Files',
  apps: 'Apps',
  plugins: 'Plugins',
  triggers: 'Triggers',
  settings: 'Settings',
  changes: 'Changes',
  notifications: 'Notifications',
};

export type ConnectionStatus = 'connected' | 'disconnected' | 'connecting' | 'stopped' | 'starting';

export type StepOutcome = 'pending' | 'success' | 'error' | 'unfinished' | 'blocked' | 'denied';

export interface Step {
  description: string;
  outcome: StepOutcome;
  tool_use_id?: string;
  parent_tool_use_id?: string;
  api_call_id?: string;
  call_event_id?: string;
  context_tokens?: number;
  context_messages?: number;
  trimmed?: boolean;
  contextCapture?: ContextCapture;
  thinkingText?: string;
}

export interface ContextCapture {
  producer: ContextProducer;
  model: string;
  context_window: number;
  sections: ContextSection[];
  tools: string[];
  estimated_total_tokens: number;
  usage?: ApiUsage;
  trimmed: boolean;
  legacy?: boolean;
  sections_stripped?: boolean;
  event_id?: string;
}

/** @deprecated kept for the `ResponseEvent.context` slot until that wiring
 *  is migrated. New code should use `ContextCapture`. */
export interface ContextAssembledData {
  sections: ContextSection[];
  tools: string[];
  model: string;
  total_chars: number;
}

type ResponseEventKind =
  | {
      type: 'text';
      md: string;
      pieces?: { at: number; seq?: number }[];
    }
  | {
      type: 'step';
      description: string;
      tool_name?: string;
      outcome: StepOutcome;
      tool_use_id?: string;
      parent_tool_use_id?: string;
      api_call_id?: string;
      children?: Extract<ResponseEvent, { type: 'step' }>[];
      finalContextCapture?: ContextCapture;
      detail?: string;
      context_tokens?: number;
      context_messages?: number;
      trimmed?: boolean;
      full?: string;
      created?: string;
      result?: string;
      result_stripped?: boolean;
      result_event_id?: string;
      args_stripped?: boolean;
      tool_channel?: 'chat' | 'coding_agent';
      call_event_id?: string;
      /** @deprecated kept for legacy backend payloads. */
      context?: ContextAssembledData;
      contextCapture?: ContextCapture;
      thinkingText?: string;
      recall?: StepRecall;
    }
  | { type: 'section_break'; channel: string }
  | {
      type: 'restart';
      reason: string;
      actor?: MessageOrigin;
    }
  | {
      type: 'fresh_session';
      reason: FreshSessionReason;
    }
  | {
      type: 'handoff_session';
      checkpoint?: string;
      progress?: string;
      opening: HandoffOpening;
      crossing?: CeilingCrossing;
      message: string;
    }
  | {
      type: 'held_message';
      held_id: string;
      text: string;
      sender: string;
      released: boolean;
    }
  | {
      type: 'cache_keep_alive';
      status: CacheKeepAliveStatus;
    }
  | {
      type: 'form_request';
      request: FormRequestEvent;
      resolution?: FormRequestOutcome;
    }
  | {
      type: 'image';
      base64: string;
      mime_type: string;
      prompt?: string;
    }
  | {
      type: 'question';
      tool_use_id: string;
      question: string;
      options: QuestionOption[];
      resolved?: AnswerKind;
    }
  | {
      type: 'permission';
      request_id: string;
      tool_use_id: string;
      tool_name: string;
      input: Record<string, unknown>;
      summary: string;
      resolved?: { allowed: boolean; reason?: string };
    }
  | {
      type: 'checkpoint';
      checkpoint_id: string;
      command: string;
      summary: string;
      reverted: boolean;
      restores: number;
      removes: number;
    }
  | {
      type: 'widget';
      app_id: string;
      params?: Record<string, unknown>;
      label?: string;
      event_id?: string;
    }
  | {
      type: 'event_wait';
      wait_id: string;
      subscriptions: EventSubscription[];
      reason: string;
      created?: string;
      expires_at: string;
      background_timing?: BackgroundWaitTiming;
      state: 'waiting' | 'matched' | 'timed_out' | 'canceled';
      matched_at?: string;
      cause?: EventWaitCancelCause;
    }
  | {
      type: 'spoken_reply';
      text: string;
      interrupted: boolean;
      follows?: boolean;
    }
  | {
      type: 'empty';
    };

export interface StepRecall {
  memories: RecalledMemory[];
  queries: string[];
}

export type ResponseEvent = ResponseEventKind & {
  seq?: number;
};

export interface Notification {
  id: string;
  task_id?: string;
  app_id?: string;
  thread_id?: string;
  event_id?: string;
  title: string;
  message: string;
  created_at: string;
  read: boolean;
  tap?: Tap;
}

export type TriggerRun =
  | { type: 'intent'; intent: string }
  | { type: 'script'; path: string };


export type { EventSubscription };

export interface TriggerInfo {
  id: string;
  name: string;
  slug: string;
  cron_expressions: string[];
  timezone: string;
  paused: boolean;
  last_run?: string;
  last_run_status?: 'ok' | 'failed';
  next_run?: string;
  next_runs?: string[];
  schedule_error?: string;
  run: TriggerRun;
  on?: EventSubscription[];
  app_id?: string;
  go_to_review?: boolean;
  group_id?: string;
  side_effect_grant?: SideEffectCategory[];
  program_grants?: { pattern: string; migrated_invocation: boolean }[];
  folder_grants?: { folder: string }[];
  program_denies?: string[];
  folder_denies?: string[];
  coding_agent_grant: boolean;
  plugin_id?: string;
  model?: string;
  reasoning_effort?: string;
  provider?: string;
}

export interface TriggerGroup {
  id: string;
  name: string;
  order: number;
  created: string;
  member_count: number;
}


export interface ThreadQueueEntry {
  id: string;
  kind: 'event-trigger' | 'cron' | 'sub-thread' | 'coding-agent' | 'user-chat';
  trigger_id?: string;
  trigger_name?: string;
  thread_id?: string;
  summary: string;
  status: 'queued' | 'admitted';
  queued_at: string;
  admitted_at?: string;
}

export interface CapacityPolicy {
  max_concurrent_total: number;
  max_concurrent_event_trigger: number;
  max_concurrent_cron: number;
  max_concurrent_sub_thread: number;
  max_concurrent_coding_agent: number;
  max_concurrent_per_trigger: number;
  max_queued_per_trigger: number;
  reserved_background: number;
  max_event_trigger_depth: number;
  max_concurrent_children_per_thread: number;
  overflow: 'drop-oldest' | 'pause-trigger';
}

export interface ThreadQueueResponse {
  entries: ThreadQueueEntry[];
  policy: CapacityPolicy;
}

export function hasNoMoreRuns(trigger: TriggerInfo): boolean {
  if (trigger.schedule_error) return false;
  return !trigger.paused && !trigger.next_run && !(trigger.on && trigger.on.length > 0);
}

export interface HistoricalTriggerInfo {
  id: string;
  name: string | null;
  last_activity: string;
}

export type TriggerType = 'schedule' | 'event' | 'hybrid';

export function deriveTriggerType(trigger: TriggerInfo): TriggerType {
  const hasCron = trigger.cron_expressions.length > 0;
  const hasEvent = !!(trigger.on && trigger.on.length > 0);
  if (hasCron && hasEvent) return 'hybrid';
  if (hasEvent) return 'event';
  return 'schedule';
}

export type AuthType =
  | 'api_key'
  | 'bearer'
  | 'basic'
  | 'password'
  | 'oauth_client'
  | 'email_password'
  | 'secret'
  | 'onepassword_service_account';

export interface CredentialInfo {
  id: string;
  service_name: string;
  base_urls: string[];
  auth_type: AuthType;
  auth_header: string;
  created_at: string;
  env_var_name?: string | null;
  secret_reference: boolean;
}

export interface EmailAccountInfo {
  id: string;
  name: string;
  email_address: string;
  imap_host: string;
  imap_port: number;
  smtp_host: string;
  smtp_port: number;
  username: string;
  use_tls: boolean;
  require_send_confirmation: boolean;
  oauth_account_id: string | null;
  created_at: string;
  updated_at: string;
}

export interface OAuthAccountInfo {
  id: string;
  provider: string;
  email: string | null;
  display_name: string | null;
  scopes: string;
  desired_scopes?: string | null;
  has_refresh_token?: boolean;
  created_at: string;
  updated_at: string;
}

export interface KnownOAuthProvider {
  id: string;
  label: string;
  base_url: string;
  auth_url: string;
  token_url: string;
  userinfo_url?: string;
  userinfo_method?: string;
  authorize_params?: string;
  redirect_uri?: string;
  client_type?: string;
  console_label?: string;
  console_url?: string;
  setup_hint?: string;
  permissions_hint?: string;
}

export interface KnownOAuthProviders {
  providers: KnownOAuthProvider[];
  default_redirect_uri: string;
}

export type AppReveal = 'on-load' | 'on-ready';

export type AppKind = 'app' | 'widget';

export interface App {
  id: string;
  name: string;
  description: string;
  icon?: string;
  reveal: AppReveal;
  kind: AppKind;
  origin_thread_id?: string;
  origin_plugin_id?: string;
  reusable: boolean;
  params?: Record<string, AppParam>;
  built_in?: boolean;
  sizes?: WidgetSize[];
  transparent?: boolean;
}

export type { TileSize, WidgetSize };

export interface AppParam {
  description: string;
  required?: boolean;
}

export interface PinnedAppEntry {
  app_id: string;
}

export interface InstalledPlugin {
  id: string;
  name: string;
  version: string;
  source?: string;
  app_id?: string;
  content: string[];
  files: string[];
  modified?: boolean;
  modified_paths?: string[];
  engine_requirement?: string;
  media: PluginMedia;
}

export interface MediaProblem {
  path: string;
  reason: string;
}

export interface PluginMedia {
  icon_url?: string;
  screenshots: string[];
  videos: string[];
  readme_url?: string;
  problems: MediaProblem[];
}


export type ConfirmVariant = 'danger' | 'default';

export interface ConfirmChoices {
  okDetail: string;
  alternative: { label: string; detail: string; onClick: () => void };
}

export type ConfirmAlternative =
  | { extraAction?: ToastAction; choices?: never }
  | { choices: ConfirmChoices; extraAction?: never };

export type ConfirmState = ConfirmAlternative & {
  visible: boolean;
  message: string;
  okLabel: string;
  title?: string;
  cancelLabel?: string;
  variant?: ConfirmVariant;
  resolve?: (value: boolean) => void;
  details?: ConfirmDetails;
  acknowledge?: boolean;
};

export interface ProgressDialogState {
  visible: boolean;
  title: string;
  message: string;
  progress: number | null;
  cancel?: ToastAction;
}

export interface ConfirmDetails {
  intro?: string;
  groups: ConfirmDetailGroup[];
}

export interface ConfirmDetailGroup {
  header: string;
  items: string[];
}

export interface PromptState {
  visible: boolean;
  message: string;
  title?: string;
  defaultValue?: string;
  placeholder?: string;
  okLabel?: string;
  cancelLabel?: string;
  multiline?: boolean;
  suggest?: () => Promise<string | null>;
  resolve?: (value: string | null) => void;
}

export type ToastType = 'success' | 'info' | 'error' | 'warning';

export interface ToastAction {
  label: string;
  onClick: () => void;
  variant?: 'danger' | 'confirm';
  deliberate?: boolean;
}

export interface StepProgress {
  done: number;
  working: number;
}

export interface ToastItem {
  id: number;
  title?: string;
  message: string;
  type: ToastType;
  key?: string;
  action?: ToastAction;
  secondaryAction?: ToastAction;
  onClick?: () => void;
  spinning?: boolean;
  progress?: number | null;
  dismissable?: boolean;
  persistent?: boolean;
  count?: number;
}

export interface CredentialRequest {
  form_request_id?: string;
  service?: string;
  base_urls?: string[];
  auth_type?: AuthType;
  prompt?: string;
  defaults?: {
    auth_url?: string;
    token_url?: string;
    userinfo_url?: string | null;
    userinfo_method?: string;
    authorize_params?: string;
    redirect_uri?: string;
  };
  env_var_name?: string;
  existing_credential_id?: string;
  adding_base_urls?: string[];
  missing?: string[];
}

export interface PluginMarketplace {
  id: string;
  name: string;
  source: string;
}

export type MarketplacePluginStatus = 'available' | 'installed' | 'update_available';

export interface MarketplacePlugin {
  marketplace_id: string;
  marketplace_name: string;
  id: string;
  name: string;
  description: string;
  version: string;
  source: string;
  manifest: Record<string, unknown>;
  content: string[];
  categories: string[];
  files_count: number;
  status: MarketplacePluginStatus;
  installed_version?: string;
  setup_thread_id?: string;
  setup_complete?: boolean;
  app_id?: string;
  modified?: boolean;
  modified_paths?: string[];
  engine_requirement?: string;
  engine_compatible: boolean;
  engine_incompatible_reason?: string;
  media: PluginMedia;
}

export interface MarketplaceScanError {
  marketplace_id: string;
  marketplace_name: string;
  source: string;
  error: string;
}

export interface MarketplaceCatalog {
  marketplaces: PluginMarketplace[];
  plugins: MarketplacePlugin[];
  errors: MarketplaceScanError[];
  scanned_at: string | null;
  scanning: boolean;
  scan_error: string | null;
}

export interface PluginInstallRequest {
  install_id: string;
  source: string;
  source_type: 'git' | 'archive';
  manifest: Record<string, unknown>;
  files: string[];
  overwrites: string[];
  removes?: string[];
  is_update?: boolean;
  setup?: string | null;
  setup_run?: PluginSetupOccasion | null;
  local_changes?: PluginLocalChange[];
  plugin_id: string;
  plugin_version: string;
  plugin_name: string;
  engine_requirement?: string | null;
  affected_triggers?: AffectedTrigger[];
}

export interface AffectedTrigger {
  trigger_id: string;
  trigger_name: string;
  paths: AffectedPath[];
}

export interface AffectedPath {
  path: string;
  reason: AffectedReason;
}

export type AffectedReason = 'named-in-run' | 'owning-app' | 'trigger-knowhow';

export type PluginLocalChangeOutcome =
  | 'merged'
  | 'conflict'
  | 'replaced'
  | 'restored'
  | 'removed';

export interface PluginLocalChange {
  path: string;
  outcome: PluginLocalChangeOutcome;
}

export interface PluginInstallReceipt {
  at: string;
  summary: string;
  installed_files: string[];
  removed_files?: string[];
  local_changes?: PluginLocalChangesResult;
  affected_triggers?: AffectedTrigger[];
}

export interface PluginUninstallReceipt {
  at: string;
  summary: string;
  files_deleted: string[];
  files_missing: string[];
  affected_triggers?: AffectedTrigger[];
}

export interface PluginUninstallRequest {
  uninstall_id: string;
  plugin_id: string;
  plugin_version: string;
  plugin_name: string;
  files_present: string[];
  files_missing: string[];
  affected_triggers?: AffectedTrigger[];
}

export interface EmailConfirmRequest {
  form_request_id?: string;
  to: string[];
  subject: string;
  body: string;
  cc?: string[];
  bcc?: string[];
  reply_to_message_id?: string;
  account: string;
  from: string;
  attachments?: string[];
  attachment_names?: string[];
}
