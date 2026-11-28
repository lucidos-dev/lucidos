import { escapeHtml } from './escapeHtml';

export function parseCsv(input: string): string[][] {
  if (input.length === 0) return [];
  const rows: string[][] = [];
  let row: string[] = [];
  let fieldParts: string[] = [];
  let inQuotes = false;
  const flushField = () => {
    row.push(fieldParts.join(''));
    fieldParts = [];
  };
  const flushRow = () => {
    flushField();
    rows.push(row);
    row = [];
  };
  let i = 0;
  while (i < input.length) {
    const ch = input[i];
    if (inQuotes) {
      if (ch === '"') {
        if (input[i + 1] === '"') {
          fieldParts.push('"');
          i += 2;
          continue;
        }
        inQuotes = false;
        i++;
        continue;
      }
      fieldParts.push(ch);
      i++;
      continue;
    }
    if (ch === '"') {
      inQuotes = true;
      i++;
      continue;
    }
    if (ch === ',') {
      flushField();
      i++;
      continue;
    }
    if (ch === '\r') {
      i += input[i + 1] === '\n' ? 2 : 1;
      flushRow();
      continue;
    }
    if (ch === '\n') {
      flushRow();
      i++;
      continue;
    }
    fieldParts.push(ch);
    i++;
  }
  if (fieldParts.length > 0 || row.length > 0) {
    flushRow();
  }
  return rows;
}

export function renderCsvTable(csv: string): string {
  const rows = parseCsv(csv);
  if (rows.length === 0) return '<p>Empty CSV</p>';

  let html = '<table class="csv-table"><thead><tr>';
  for (const h of rows[0]) {
    html += `<th>${escapeHtml(h)}</th>`;
  }
  html += '</tr></thead><tbody>';

  for (let i = 1; i < rows.length; i++) {
    html += '<tr>';
    for (const c of rows[i]) {
      html += `<td>${escapeHtml(c)}</td>`;
    }
    html += '</tr>';
  }
  html += '</tbody></table>';
  return html;
}
