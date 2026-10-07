/** Recur Simplify mode: nil/omitted settings → on. */
export function simplifyEnabled(s: { simplify?: boolean | null } | null | undefined): boolean {
  return s?.simplify !== false
}

/** Session chip: an explicit session flag wins, otherwise the Settings default. */
export function simplifyFromSelection(
  sel: { simplify?: boolean | null } | null | undefined,
  settings: { simplify?: boolean | null } | null | undefined,
): boolean {
  if (typeof sel?.simplify === 'boolean') return sel.simplify
  return simplifyEnabled(settings)
}
