/** Recur Simplify mode: nil/omitted settings → on. */
export function simplifyEnabled(s: { simplify?: boolean | null } | null | undefined): boolean {
  return s?.simplify !== false
}
