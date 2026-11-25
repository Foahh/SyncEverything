const CONSTRAINT_SPLIT_RE = /[,，]/;

export function isValidNoteLabel(value: string): boolean {
  return value.trim().length >= 1;
}

export function parseNoteConstraints(raw: string): string[] {
  return raw
    .split(CONSTRAINT_SPLIT_RE)
    .map((item) => item.trim())
    .filter(Boolean);
}
