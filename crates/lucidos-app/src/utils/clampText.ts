export function clampText(text: string, maxChars: number): string {
  if (text.length <= maxChars) return text;
  const points = [...text];
  if (points.length <= maxChars) return text;
  return `${points.slice(0, maxChars - 1).join('').trimEnd()}…`;
}
