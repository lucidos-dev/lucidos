import { describe, expect, it } from 'vitest';
import handoffScript from '../../../public/outbox-handoff.js?raw';
import { PENDING_UPLOADS_DATABASE, UNSENT_MESSAGES_DATABASE } from '../../store/deviceDatabase';
import { PENDING_UPLOADS_STORE } from '../../store/pendingUploadBackend';
import { RESTORE_GENERATION_DATABASE, RESTORE_OBSERVATIONS_STORE } from '../../store/restoreGenerationRecords';
import { UPLOAD_BYTES } from '../../store/uploadByteStore';
import { UNSENT_MESSAGES_STORE } from '../../store/unsentMessageRecords';
import {
  OUTBOX_EXPORT_VERSION,
  OUTBOX_HANDOFF_COMMANDS,
  OUTBOX_HANDOFF_MESSAGES,
  OUTBOX_IMPORT_PATH,
  decodeNativeOutbox,
  handoffSourceOrigins,
  outboxRecordCount,
  parseOutboxExport,
} from '../outboxHandoff';
import { exportOf as typedExportOf, unsentRecord, uploadRecord } from './fixtures';

function exportOf(workspaces: unknown[]) {
  return typedExportOf(workspaces as never);
}

describe('outbox hand-off protocol', () => {
  it('reads a well-formed export and counts every record', () => {
    const parsed = parseOutboxExport(exportOf([
      { workspaceId: 'dev', unsentMessages: [unsentRecord('e1')], pendingUploads: [uploadRecord('u1')] },
      { workspaceId: null, unsentMessages: [unsentRecord('e2')], pendingUploads: [] },
    ]));
    expect(parsed).not.toBeNull();
    expect(outboxRecordCount(parsed!)).toBe(3);
  });

  it('counts each restore journal observation, and decodes its bytes from the native store', () => {
    const journal = {
      observations: [['workspace', { generation: 3, recoveries: [] }]],
      uploadBytes: [[[['f', 1], 0], { bytes: 'AQL6/w==' }]],
    };
    const decoded = decodeNativeOutbox(JSON.stringify(exportOf([
      { workspaceId: 'dev', unsentMessages: [], pendingUploads: [], restoreJournal: journal },
    ])));
    expect(decoded).not.toBeNull();
    expect(outboxRecordCount(decoded!)).toBe(1);
    const [, payload] = decoded!.workspaces[0].restoreJournal!.uploadBytes[0] as [unknown, { bytes: ArrayBuffer }];
    expect([...new Uint8Array(payload.bytes)]).toEqual([1, 2, 250, 255]);
    expect(parseOutboxExport(exportOf([
      { workspaceId: 'dev', unsentMessages: [], pendingUploads: [], restoreJournal: { observations: 'no' } },
    ]))).toBeNull();
  });

  it('refuses another kind, another version or a malformed record', () => {
    const workspace = { workspaceId: 'dev', unsentMessages: [unsentRecord('e1')], pendingUploads: [] };
    expect(parseOutboxExport({ ...exportOf([workspace]), kind: 'other' })).toBeNull();
    expect(parseOutboxExport({ ...exportOf([workspace]), version: 2 })).toBeNull();
    expect(parseOutboxExport(exportOf([{ ...workspace, unsentMessages: [{ eventId: 1 }] }]))).toBeNull();
    expect(parseOutboxExport(exportOf([{ ...workspace, pendingUploads: [{ ...uploadRecord('u1'), bytes: 'x' }] }]))).toBeNull();
    expect(parseOutboxExport(null)).toBeNull();
  });

  it('refuses a native payload that is not an export this version reads', () => {
    expect(decodeNativeOutbox('not json')).toBeNull();
    expect(decodeNativeOutbox(JSON.stringify({ kind: 'other', workspaces: [] }))).toBeNull();
    const upload = { ...uploadRecord('u1'), bytes: '%%%' };
    expect(decodeNativeOutbox(JSON.stringify(exportOf([
      { workspaceId: 'dev', unsentMessages: [], pendingUploads: [upload] },
    ])))).toBeNull();
  });

  it('hears only loopback http origins on the same port', () => {
    expect(handoffSourceOrigins('5252')).toEqual([
      'http://localhost:5252',
      'http://127.0.0.1:5252',
      'http://[::1]:5252',
    ]);
  });

  it('gives the plain-JS hand-off page the same names as this module', () => {
    const window: { lucidosOutboxHandoff?: { names: Record<string, unknown> } } = {};
    new Function('window', handoffScript)(window);
    const { names } = window.lucidosOutboxHandoff!;
    expect(names.importPath).toBe(OUTBOX_IMPORT_PATH);
    expect(names.messages).toEqual(OUTBOX_HANDOFF_MESSAGES);
    expect(names.version).toBe(OUTBOX_EXPORT_VERSION);
    expect(names.nativeStore).toBe(OUTBOX_HANDOFF_COMMANDS.store);
    expect(names.databases).toEqual({
      unsentMessages: { base: UNSENT_MESSAGES_DATABASE, store: UNSENT_MESSAGES_STORE },
      pendingUploads: { base: PENDING_UPLOADS_DATABASE, store: PENDING_UPLOADS_STORE },
    });
    expect(names.restoreJournal).toEqual({
      base: RESTORE_GENERATION_DATABASE,
      stores: { observations: RESTORE_OBSERVATIONS_STORE, uploadBytes: UPLOAD_BYTES },
    });
  });
});
