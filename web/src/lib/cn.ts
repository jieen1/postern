/** Tiny classname joiner (no clsx dependency needed for the scaffold). */
export function cn(...parts: Array<string | false | null | undefined>): string {
  return parts.filter(Boolean).join(' ');
}
