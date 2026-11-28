export function openFold(host: HTMLElement, label: string) {
  const toggle = [...host.querySelectorAll<HTMLButtonElement>('button[aria-expanded="false"]')]
    .find((b) => b.textContent?.trim() === label);
  if (!toggle) throw new Error(`No closed fold labelled "${label}"`);
  toggle.click();
}
