/** True when keyboard focus is already inside a terminal dock.
 *
 *  A workspace switch remounts the dock (`{#key workspaceId}`). The new pane
 *  must not steal keys from the editor or agent; a freshly spawned tab still
 *  takes them because spawn() parks focus on the dock first.
 *
 *  Pass `within` (the pane host) so a dock in another view root — `#v-detail`
 *  vs `#v-prdetail` can both be mounted — cannot authorise this pane to take
 *  keys. Without it, any `.term-dock` in the document counts. */
export function termDockOwnsFocus(within?: Element | null): boolean {
  const ae = document.activeElement
  if (!(ae instanceof Element)) return false
  const dock = within?.closest('.term-dock')
  return dock ? dock.contains(ae) : !!ae.closest('.term-dock')
}
