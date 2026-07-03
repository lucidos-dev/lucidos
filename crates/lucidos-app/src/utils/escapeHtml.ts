const el = document.createElement('div');

export function escapeHtml(text: string): string {
  el.textContent = text;
  return el.innerHTML;
}

export function stripHtml(html: string): string {
  el.innerHTML = html;
  return el.textContent || el.innerText || '';
}
