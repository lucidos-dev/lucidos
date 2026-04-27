import { escapeHtml } from './escapeHtml';

export function renderCsvTable(csv: string): string {
  const lines = csv.trim().split('\n');
  if (lines.length === 0) return '<p>Empty CSV</p>';

  let html = '<table class="csv-table"><thead><tr>';
  const headers = lines[0].split(',');
  for (const h of headers) {
    html += `<th>${escapeHtml(h.trim())}</th>`;
  }
  html += '</tr></thead><tbody>';

  for (let i = 1; i < lines.length; i++) {
    const cells = lines[i].split(',');
    html += '<tr>';
    for (const c of cells) {
      html += `<td>${escapeHtml(c.trim())}</td>`;
    }
    html += '</tr>';
  }
  html += '</tbody></table>';
  return html;
}
