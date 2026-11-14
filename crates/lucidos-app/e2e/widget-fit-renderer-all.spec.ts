import { fileURLToPath } from 'node:url';
import { randomUUID } from 'node:crypto';
import { test, expect } from './fixtures';
import { apiRequest, assertHealthy, navigateToApp, newThread, pickComposeDestination, sendMessage, waitForResponse } from './helpers';
import { psql } from './db-helpers';

const FAKE = fileURLToPath(new URL('./fake-claude-code/claude.mjs', import.meta.url));
const PREF = 'coding_agent_claude_path';
const html = (body: string) => '<!doctype html><html><head><script src="/api/v1/sdk-prefs.js"></script>'
  + '<link rel="stylesheet" href="/api/v1/sdk-iframe.css"><script src="/api/v1/sdk.js"></script>'
  + '</head><body style="margin:0;padding:0.25rem">' + body + '</body></html>';

test('a verified agent renderer reports real SDK layout without Device authority', async ({ page }) => {
  test.setTimeout(240000);
  await assertHealthy(page);
  const preferenceScope = `key IN ('${PREF}', 'ui-scale') AND device_id IS NULL`;
  const oldPreferences = psql(`SELECT COALESCE(json_agg(p), '[]'::json) FROM preferences p WHERE ${preferenceScope}`);
  const prefix = `e2e-fit-${randomUUID().slice(0, 8)}`;
  const widgets = [
    { id: `${prefix}-clean`, body: html('<span>Ready</span>') },
    { id: `${prefix}-cut`, body: html(`<div style="white-space:nowrap">${'Important fixture data '.repeat(100)}</div>`) },
    { id: `${prefix}-params`, body: html('<span>Choose an item</span>'), params: { item: { required: true } } },
    { id: `${prefix}-failed`, body: '<!doctype html><html><body>No SDK</body></html>' },
    { id: `${prefix}-silent`, body: html('<span>Ready</span>'), noReport: true },
  ];
  const written: string[] = [];
  try {
    expect((await apiRequest(page).put(`/api/v1/preferences?key=${PREF}`, { data: { value: FAKE } })).ok()).toBe(true);
    expect((await apiRequest(page).put('/api/v1/preferences?key=ui-scale', { data: { value: '200' } })).ok()).toBe(true);
    for (const widget of widgets) {
      for (const [file, body] of [
        ['manifest.json', JSON.stringify({ name: 'Fit fixture', kind: 'widget', sizes: ['small', 'medium', 'large', 'xl'], params: widget.params ?? {} })],
        ['index.html', widget.body],
      ]) {
        const path = `apps/${widget.id}/${file}`;
        written.push(path);
        expect((await apiRequest(page).put(`/api/v1/data/${path}`, { headers: { 'Content-Type': 'text/plain' }, data: body })).ok()).toBe(true);
      }
    }
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);
    const browser = test.info().project.name === 'mobile-webkit' ? 'webkit' : 'chromium';
    await sendMessage(page, `Probe fit renderer: ${JSON.stringify({ browser, widgets: widgets.map(({ id, noReport }) => ({ id, noReport })) })}`);
    const response = await waitForResponse(page, 180000);
    await expect(response).toContainText('"verified":true', { timeout: 180000 });
    const proof = JSON.parse((await response.locator('pre code').textContent())!.trim());
    expect(proof.responder).toBe('agent-renderer');
    expect(proof.results).toHaveLength(widgets.length);
    for (const result of proof.results) {
      expect(result.operationStatus, result.note).toBe(200);
      if (result.app.endsWith('-silent')) {
        expect(result.status).toBeNull();
        expect(result.note).toContain('not checked');
        continue;
      }
      expect(result.status, result.note).toBe(200);
      expect(result.refused).toEqual([409, 404]);
      expect(result.replayStatus).toBe(409);
      expect(result.reports).toHaveLength(1);
      const boxes = result.reports[0].boxes;
      expect(boxes).toHaveLength(8);
      for (const surface of ['desk-tile', 'phone-column']) {
        expect(boxes.filter((box: { surface: string }) => box.surface === surface).map((box: { size: string }) => box.size)).toEqual(['small', 'medium', 'large', 'xl']);
      }
      expect(result.note).toContain('200% UI scale');
    }
    expect(proof.results[0].note).toContain(': fits');
    expect(proof.results[1].note).toContain('content is cut off');
    expect(proof.results[2].note).toContain('not checked with params');
    expect(proof.results[3].note).toContain('could not be measured');
    await test.info().attach('fit-renderer-measurements', {
      body: JSON.stringify(proof), contentType: 'application/json',
    });
  } finally {
    // A timed-out Playwright request cannot restore shared fixture state.
    // Restore it before any awaited cleanup, even when the test budget expired.
    psql(`BEGIN; DELETE FROM preferences WHERE ${preferenceScope};
      INSERT INTO preferences SELECT * FROM json_populate_recordset(NULL::preferences, '${oldPreferences.replaceAll("'", "''")}'::json); COMMIT;`);
    for (const path of written) await apiRequest(page).delete(`/api/v1/data/${path}`);
  }
});
