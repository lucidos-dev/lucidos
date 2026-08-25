// Barrel for the HTTP API client. Implementation lives in ./client/*.ts,
// split by domain; this file re-exports the full surface so importers keep
// using `from '../api/client'` unchanged. See CLAUDE.md "API URL Conventions".

export * from './client/_core';
export * from './client/chat';
export * from './client/changes';
export * from './client/apps';
export * from './client/data';
export * from './client/frontendPreview';
export * from './client/triggers';
export * from './client/threadQueue';
export * from './client/threads';
export * from './client/settings';
export * from './client/models';
export * from './client/mcp';
export * from './client/pluginMarketplaces';
export * from './client/releaseNotices';
export * from './client/webhooks';
