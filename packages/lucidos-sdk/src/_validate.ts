export function assertPlainObject(name: string, value: unknown): void {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new TypeError(`${name} must be a plain object`);
  }
}

export function assertArray(name: string, value: unknown): void {
  if (!Array.isArray(value)) {
    throw new TypeError(`${name} must be an array`);
  }
}

export function assertString(name: string, value: unknown): void {
  if (typeof value !== 'string') {
    throw new TypeError(`${name} must be a string`);
  }
}
