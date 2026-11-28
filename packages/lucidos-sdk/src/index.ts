import { apiUrl, configure, SdkError } from './_fetch';
import { media } from './media';
import { data } from './data';
import { events } from './events';
import { triggers } from './triggers';
import { preferences } from './preferences';
import { priceTable } from './priceTable';
import { notifications } from './notifications';
import { apps } from './apps';
import { threads } from './threads';
import { ui } from './ui';
import { sse } from './sse';
import { utils } from './utils';
import { capture } from './capture';
import { proxy } from './proxy';
import { modelRequest } from './modelRequest';
import { oauth } from './oauth';
import { request } from './request';
import { storage } from './storage';
import { params } from './params';
import { widget } from './widgetFrame';

export const lucidos = {
  configure,
  apiUrl,
  request,
  data,
  media,
  events,
  triggers,
  preferences,
  priceTable,
  notifications,
  apps,
  threads,
  ui,
  sse,
  utils,
  proxy,
  modelRequest,
  oauth,
  storage,
  params,
  widget,
  _capture: capture,
};

export { SdkError };
export type * from './types';
export type { StorageFailure } from './storage';
export type { UserRef, ThreadUsers, ThreadUsersList, ThreadUsersSummary } from './threadUsers';
export type { LivenessState, ThreadLiveness } from './threads';

export { appMayCall, appReachableMethods, normalizeSuffix, pathMatchesPattern } from './appReach';
export { APP_REACHABLE_ROUTES } from './generated/app-reach';
export { parseAppId } from './scroll';
export { WIDGET_PARAMS_QUERY, isWidgetParams } from './params';
export type { WidgetParams } from './params';
export * from './appStorage';

export { CODING_AGENT_TARGETS, NAVIGATE_TARGETS, SETTINGS_VIEW_TARGETS } from './generated/navigate-targets';

export type { ModelPickerValue, ModelPickerFilter, ModelPickerOptions, ModelPickerInstance } from './modelPicker';
export type { ModelMessage, ModelRequestOptions, ModelRequest } from './modelRequest';

export type { MediaPoster } from './media';
