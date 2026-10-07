/** App chrome icon registry. Add a key here — Icon.svelte looks it up. */

export type IconDef = {
  /** Inner SVG markup (paths/shapes only — no outer <svg>). Trusted constants only. */
  body: string
  /** Default stroke width when the caller doesn't pass one. */
  strokeWidth?: number
  /** Defaults to 0 0 24 24. */
  viewBox?: string
  /** Filled glyph (Tabler filled, etc.) — uses fill=currentColor, no stroke. */
  filled?: boolean
}

export const icons = {
  plus: {
    strokeWidth: 1.9,
    body: '<path d="M5 12h14"/><path d="M12 5v14"/>',
  },
  // lucide box — Open in palette header
  box: {
    strokeWidth: 1.9,
    body:
      '<path d="M21 8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16Z"/>' +
      '<path d="m3.3 7 8.7 5 8.7-5"/>' +
      '<path d="M12 22V12"/>',
  },
  // GitHub mark — Open in GitHub (PR titlebar)
  github: {
    filled: true,
    body: '<path d="M12 2a10 10 0 0 0-3.2 19.5c.5.1.7-.2.7-.5v-1.7c-2.8.6-3.4-1.3-3.4-1.3-.5-1.1-1.1-1.4-1.1-1.4-.9-.6.1-.6.1-.6 1 .1 1.5 1 1.5 1 .9 1.5 2.3 1.1 2.9.8.1-.6.3-1.1.6-1.3-2.2-.3-4.6-1.1-4.6-4.9 0-1.1.4-2 1-2.7-.1-.3-.4-1.3.1-2.7 0 0 .8-.3 2.7 1a9.4 9.4 0 0 1 5 0c1.9-1.3 2.7-1 2.7-1 .5 1.4.2 2.4.1 2.7.6.7 1 1.6 1 2.7 0 3.8-2.3 4.6-4.6 4.9.4.3.7.9.7 1.9v2.8c0 .3.2.6.7.5A10 10 0 0 0 12 2z"/>',
  },
  // Linear logomark — Linear MCP tool rows
  linear: {
    filled: true,
    body: '<path d="M2.886 4.18A11.982 11.982 0 0 1 11.99 0C18.624 0 24 5.376 24 12.009c0 3.64-1.62 6.903-4.18 9.105L2.887 4.18ZM1.817 5.626l16.556 16.556c-.524.33-1.075.62-1.65.866L.951 7.277c.247-.575.537-1.126.866-1.65ZM.322 9.163l14.515 14.515c-.71.172-1.443.282-2.195.322L0 11.358a12 12 0 0 1 .322-2.195Zm-.17 4.862 9.823 9.824a12.02 12.02 0 0 1-9.824-9.824Z"/>',
  },
  // lucide hard-drive — Local PR source
  'hard-drive': {
    strokeWidth: 2,
    body:
      '<path d="M10 16h.01"/>' +
      '<path d="M2.212 11.577a2 2 0 0 0-.212.896V18a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-5.527a2 2 0 0 0-.212-.896L18.55 5.11A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z"/>' +
      '<path d="M21.946 12.013H2.054"/>' +
      '<path d="M6 16h.01"/>',
  },
  // tabler queue-pop-out — Open in (titlebar + palette)
  'queue-pop-out': {
    strokeWidth: 2,
    body:
      '<path d="M11 5h-6a2 2 0 0 0 -2 2v12a2 2 0 0 0 2 2h12a2 2 0 0 0 2 -2v-6"/>' +
      '<path d="M3 13h6a2 2 0 0 1 2 2v6"/>' +
      '<path d="M16 3h5v5"/>' +
      '<path d="M21 3l-6 6"/>',
  },
  // lucide pin — Open in default
  pin: {
    strokeWidth: 1.9,
    body:
      '<path d="M12 17v5"/>' +
      '<path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z"/>',
  },
  minus: {
    strokeWidth: 2,
    body: '<path d="M6 12h12"/>',
  },
  play: {
    strokeWidth: 1.9,
    body: '<path d="M8 5.5v13l11-6.5-11-6.5z"/>',
  },
  // Lucide "command" — ⌘-key / Run Command palette affordance.
  command: {
    body: '<path d="M15 6v12a3 3 0 1 0 3-3H6a3 3 0 1 0 3 3V6a3 3 0 1 0-3 3h12a3 3 0 1 0-3-3"/>',
  },
  pause: {
    body: '<rect x="6" y="5" width="4" height="14" rx="1"/><rect x="14" y="5" width="4" height="14" rx="1"/>',
  },
  'circle-pause': {
    body: '<circle cx="12" cy="12" r="9"/><path d="M10 9v6M14 9v6"/>',
  },
  square: {
    body: '<rect x="6" y="6" width="12" height="12" rx="1.5"/>',
  },
  // Media-stop: filled so it reads as a solid control at 15px chrome size.
  'square-fill': {
    body: '<rect x="6" y="6" width="12" height="12" rx="1.5" fill="currentColor" stroke="none"/>',
  },
  // lucide square-check — selected run in a schedule list
  'square-check': {
    body: '<rect width="18" height="18" x="3" y="3" rx="2"/><path d="m16 9-5.5 5.5L8 12"/>',
  },
  'octagon-x': {
    body: '<path d="m15 9-6 6"/><path d="M2.586 16.726A2 2 0 0 1 2 15.312V8.688a2 2 0 0 1 .586-1.414l4.688-4.688A2 2 0 0 1 8.688 2h6.624a2 2 0 0 1 1.414.586l4.688 4.688A2 2 0 0 1 22 8.688v6.624a2 2 0 0 1-.586 1.414l-4.688 4.688a2 2 0 0 1-1.414.586H8.688a2 2 0 0 1-1.414-.586z"/><path d="m9 9 6 6"/>',
  },
  ban: {
    strokeWidth: 1.9,
    body: '<circle cx="12" cy="12" r="10"/><path d="M4.929 4.929 19.07 19.071"/>',
  },
  // lucide circle-slash — skipped GitHub check
  'circle-slash': {
    body: '<circle cx="12" cy="12" r="10"/><line x1="9" x2="15" y1="15" y2="9"/>',
  },
  check: {
    body: '<path d="M5 12l4 4L19 6"/>',
  },
  x: {
    body: '<path d="M6 6l12 12M18 6L6 18"/>',
  },
  'circle-check': {
    body: '<circle cx="12" cy="12" r="9"/><path d="M8 12l2.5 2.5L16 9"/>',
  },
  'circle-alert': {
    body: '<circle cx="12" cy="12" r="9"/><path d="M12 8v5M12 16h.01"/>',
  },
  info: {
    body: '<circle cx="12" cy="12" r="9"/><path d="M12 11v6M12 8h.01"/>',
  },
  'triangle-alert': {
    body:
      '<path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3"/>' +
      '<path d="M12 9v4"/>' +
      '<path d="M12 17h.01"/>',
  },
  maximize: {
    strokeWidth: 1.9,
    body: '<path d="M10 19H5v-5M14 5h5v5"/>',
  },
  minimize: {
    strokeWidth: 1.9,
    body: '<path d="M5 14h5v5M19 10h-5V5"/>',
  },
  // lucide feather — Simplify mode composer chip
  feather: {
    strokeWidth: 1.9,
    body:
      '<path d="M14.086 18.412A2 2 0 0112.67 19H5v-7.672a2 2 0 01.586-1.414L11.75 3.75a6 6 0 118.49 8.49z"/>' +
      '<path d="M16 8 2 22"/>' +
      '<path d="M17.488 15H9"/>',
  },
  'panel-bottom': {
    strokeWidth: 1.9,
    body:
      '<rect width="18" height="18" x="3" y="3" rx="2"/>' +
      '<path d="M3 15h18"/>' +
      '<path d="m15 8-3 3-3-3"/>',
  },
  'panel-left': {
    strokeWidth: 1.75,
    body: '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M9 4v16"/>',
  },
  'panel-right': {
    strokeWidth: 1.75,
    body: '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M15 4v16"/>',
  },
  columns: {
    strokeWidth: 1.75,
    body: '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M12 4v16"/>',
  },
  // lucide square-split-horizontal — editor | terminal
  'square-split-horizontal': {
    strokeWidth: 2,
    body:
      '<path d="M12 2v20"/>' +
      '<path d="M16 3h3a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2h-3"/>' +
      '<path d="M8 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3"/>',
  },
  // lucide columns-2 — side-by-side diff
  'columns-2': {
    strokeWidth: 2,
    body: '<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M12 3v18"/>',
  },
  // lucide rows-2 — unified diff
  'rows-2': {
    strokeWidth: 2,
    body: '<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M3 12h18"/>',
  },
  // lucide rows-3 — stacked / whole-branch diffs
  'rows-3': {
    strokeWidth: 2,
    body: '<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M21 9H3"/><path d="M21 15H3"/>',
  },
  // tabler switch-3 — switch_mode tool row
  'switch-3': {
    strokeWidth: 2,
    body:
      '<path d="M3 17h2.397a5 5 0 0 0 4.096 -2.133l.177 -.253m3.66 -5.227l.177 -.254a5 5 0 0 1 4.096 -2.133h3.397"/>' +
      '<path d="M18 4l3 3l-3 3"/>' +
      '<path d="M3 7h2.397a5 5 0 0 1 4.096 2.133l4.014 5.734a5 5 0 0 0 4.096 2.133h3.397"/>' +
      '<path d="M18 20l3 -3l-3 -3"/>',
  },
  // lucide a-large-small — rename_session tool row
  'a-large-small': {
    strokeWidth: 2,
    body:
      '<path d="m15 16 2.536-7.328a1.02 1.02 1 0 1 1.928 0L22 16"/>' +
      '<path d="M15.697 14h5.606"/>' +
      '<path d="m2 16 4.039-9.69a.5.5 0 0 1 .923 0L11 16"/>' +
      '<path d="M3.304 13h6.392"/>',
  },
  'columns-3': {
    strokeWidth: 1.8,
    body: '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M9 4v16M15 4v16"/>',
  },
  'layout-list': {
    strokeWidth: 1.8,
    body: '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M3 10h18M9 10v10"/>',
  },
  terminal: {
    strokeWidth: 1.75,
    body:
      '<rect x="3" y="4" width="18" height="16" rx="2"/>' +
      '<path d="M7 9l3 3-3 3M13 15h4"/>',
  },
  'square-terminal': {
    strokeWidth: 2.2,
    body:
      '<rect x="4" y="4" width="16" height="16" rx="2"/>' +
      '<path d="M9 9l-2 3 2 3M15 9l2 3-2 3"/>',
  },
  'chevrons-right': {
    body: '<path d="M4 7l5 5-5 5M11 17h9"/>',
  },
  grid: {
    strokeWidth: 1.75,
    body:
      '<rect x="3" y="3" width="7" height="7" rx="1.5"/>' +
      '<rect x="14" y="3" width="7" height="7" rx="1.5"/>' +
      '<rect x="3" y="14" width="7" height="7" rx="1.5"/>' +
      '<rect x="14" y="14" width="7" height="7" rx="1.5"/>',
  },
  inbox: {
    body:
      '<polyline points="22 12 16 12 14 15 10 15 8 12 2 12"/>' +
      '<path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z"/>',
  },
  'inbox-empty': {
    strokeWidth: 1.8,
    body: '<path d="M4 5h16v9h-5l-3 3-3-3H4z"/><path d="M9 9h6M9 12h4"/>',
  },
  // lucide git-pull-request-arrow
  'git-pull-request': {
    strokeWidth: 2,
    body:
      '<circle cx="5" cy="6" r="3"/>' +
      '<path d="M5 9v12"/>' +
      '<circle cx="19" cy="18" r="3"/>' +
      '<path d="m15 9-3-3 3-3"/>' +
      '<path d="M12 6h5a2 2 0 0 1 2 2v7"/>',
  },
  // lucide git-pull-request-draft
  'git-pull-request-draft': {
    strokeWidth: 2,
    body:
      '<circle cx="18" cy="18" r="3"/>' +
      '<circle cx="6" cy="6" r="3"/>' +
      '<path d="M18 6V5"/>' +
      '<path d="M18 11v-1"/>' +
      '<line x1="6" x2="6" y1="9" y2="21"/>',
  },
  // lucide git-pull-request-closed
  'git-pull-request-closed': {
    strokeWidth: 2,
    body:
      '<path d="m15.5 3.5 5 5"/>' +
      '<path d="m15.5 8.5 5-5"/>' +
      '<path d="M18 11.62V15"/>' +
      '<path d="M6 9v12"/>' +
      '<circle cx="18" cy="18" r="3"/>' +
      '<circle cx="6" cy="6" r="3"/>',
  },
  // mirror of git-branch — out right from the top, then down (tip inset)
  'git-merge': {
    strokeWidth: 2,
    body:
      '<circle cx="6" cy="6" r="3"/>' +
      '<circle cx="6" cy="18" r="3"/>' +
      '<circle cx="18" cy="16" r="3"/>' +
      '<path d="M6 9v6M9 6h6a3 3 0 0 1 3 3v4"/>',
  },
  // lucide-weight git-branch — out right, then up (tip inset, no arrowhead)
  'git-branch': {
    strokeWidth: 2,
    body:
      '<circle cx="6" cy="6" r="3"/>' +
      '<circle cx="6" cy="18" r="3"/>' +
      '<circle cx="18" cy="8" r="3"/>' +
      '<path d="M6 9v6M9 18h6a3 3 0 0 0 3-3V11"/>',
  },
  'git-commit': {
    // lucide git-commit-vertical
    body: '<path d="M12 3v6"/><circle cx="12" cy="12" r="3"/><path d="M12 15v6"/>',
  },
  // house-style git-fork drawn as a FAN-OUT — one parent node splitting into
  // two children (fan-out wave card). Same weights as git-branch/git-merge:
  // r=3 nodes, connectors meeting the node edges, 3-unit corner arcs.
  'git-fork': {
    strokeWidth: 2,
    body:
      '<circle cx="12" cy="6" r="3"/>' +
      '<circle cx="6" cy="18" r="3"/>' +
      '<circle cx="18" cy="18" r="3"/>' +
      '<path d="M12 9v3M12 12H9a3 3 0 0 0-3 3M12 12h3a3 3 0 0 1 3 3"/>',
  },
  // lucide split — fork a chat from a finished agent turn
  split: {
    body:
      '<path d="M16 3h5v5"/>' +
      '<path d="M8 3H3v5"/>' +
      '<path d="M12 22v-8.3a4 4 0 0 0-1.172-2.872L3 3"/>' +
      '<path d="m15 9 6-6"/>',
  },
  'git-line': {
    body: '<circle cx="6" cy="6" r="2.5"/><circle cx="6" cy="18" r="2.5"/><path d="M6 8.5v7"/>',
  },
  settings: {
    body:
      '<path d="M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915"/>' +
      '<circle cx="12" cy="12" r="3"/>',
  },
  // Lucide "sliders-horizontal" — General settings nav.
  'sliders-horizontal': {
    body:
      '<path d="M10 5H3"/><path d="M12 19H3"/><path d="M14 3v4"/><path d="M16 17v4"/>' +
      '<path d="M21 12h-9"/><path d="M21 19h-5"/><path d="M21 5h-7"/><path d="M8 10v4"/><path d="M8 12H3"/>',
  },
  help: {
    body:
      '<circle cx="12" cy="12" r="9"/>' +
      '<path d="M9.5 9a2.5 2.5 0 1 1 3 2.5c-.8.4-1.5 1-1.5 2M12 17h.01"/>',
  },
  search: {
    strokeWidth: 1.75,
    body: '<circle cx="11" cy="11" r="6"/><path d="M16 16l4 4"/>',
  },
  // lucide fold-vertical / unfold-vertical — review collapse-all
  'fold-vertical': {
    body:
      '<path d="M12 22v-6"/>' +
      '<path d="M12 8V2"/>' +
      '<path d="M4 12H2"/>' +
      '<path d="M10 12H8"/>' +
      '<path d="M16 12h-2"/>' +
      '<path d="M22 12h-2"/>' +
      '<path d="m15 19-3 3-3-3"/>' +
      '<path d="m15 5-3-3-3 3"/>',
  },
  'unfold-vertical': {
    body:
      '<path d="M12 8V2"/>' +
      '<path d="M12 22v-6"/>' +
      '<path d="M4 12H2"/>' +
      '<path d="M10 12H8"/>' +
      '<path d="M16 12h-2"/>' +
      '<path d="M22 12h-2"/>' +
      '<path d="m15 5-3 3-3-3"/>' +
      '<path d="m15 19-3-3-3 3"/>',
  },
  'case-sensitive': {
    // lucide case-sensitive — editor find/replace match-case toggle
    body:
      '<path d="m2 16 4.039-9.69a.5.5 0 0 1 .923 0L11 16"/>' +
      '<path d="M22 9v7"/>' +
      '<path d="M3.304 13h6.392"/>' +
      '<circle cx="18.5" cy="12.5" r="3.5"/>',
  },
  regex: {
    // lucide regex — editor find/replace regexp toggle
    body:
      '<path d="M17 3v10"/>' +
      '<path d="m12.67 5.5 8.66 5"/>' +
      '<path d="m12.67 10.5 8.66-5"/>' +
      '<path d="M9 17a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v2a2 2 0 0 0 2 2h2a2 2 0 0 0 2-2v-2z"/>',
  },
  'whole-word': {
    // lucide whole-word — editor find/replace whole-word toggle
    body:
      '<circle cx="7" cy="12" r="3"/>' +
      '<path d="M10 9v6"/>' +
      '<circle cx="17" cy="12" r="3"/>' +
      '<path d="M14 7v8"/>' +
      '<path d="M22 17v1c0 .5-.5 1-1 1H3c-.5 0-1-.5-1-1v-1"/>',
  },
  'chevron-down': {
    body: '<path d="M6 9l6 6 6-6"/>',
  },
  'chevron-left': {
    body: '<path d="M15 18l-6-6 6-6"/>',
  },
  'chevron-right': {
    body: '<path d="M9 6l6 6-6 6"/>',
  },
  replace: {
    // find/replace row glyph (swap arrows)
    body:
      '<path d="M4 7h11"/><path d="M12 4l3 3-3 3"/><path d="M20 17H9"/><path d="M12 14l-3 3 3 3"/>',
  },
  'chevrons-up-down': {
    strokeWidth: 2.2,
    body: '<path d="M7 9l5-5 5 5M7 15l5 5 5-5"/>',
  },
  filter: {
    body: '<path d="M3 5h18l-7 8v6l-4-2v-4z"/>',
  },
  lock: {
    body: '<rect x="5" y="11" width="14" height="9" rx="2"/><path d="M8 11V8a4 4 0 0 1 8 0v3"/>',
  },
  'clipboard-check': {
    body:
      '<path d="M9 11l3 3 8-8"/>' +
      '<path d="M21 12v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h9"/>',
  },
  refresh: {
    strokeWidth: 1.75,
    body:
      '<path d="M4 12a8 8 0 0 1 14-5M20 12a8 8 0 0 1-14 5"/>' +
      '<path d="M18 4v5h-5M6 20v-5h5"/>',
  },
  'refresh-cw': {
    body: '<path d="M21 12a9 9 0 1 1-3-6.7"/><path d="M21 4v4h-4"/>',
  },
  // lucide redo-2 — chat error Retry
  'redo-2': {
    body: '<path d="m15 14 5-5-5-5"/><path d="M20 9H9.5A5.5 5.5 0 0 0 4 14.5A5.5 5.5 0 0 0 9.5 20H13"/>',
  },
  'refresh-ccw': {
    body:
      '<path d="M4 4v6h6M20 20v-6h-6"/>' +
      '<path d="M19 9a7 7 0 0 0-13-2M5 15a7 7 0 0 0 13 2"/>',
  },
  loader: {
    strokeWidth: 3,
    body: '<path d="M12 3a9 9 0 1 1-9 9"/>',
  },
  'arrow-up': {
    body: '<path d="M12 19V5M5 12l7-7 7 7"/>',
  },
  upload: {
    // lucide upload — push HEAD to the tracking remote
    body: '<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4M17 8l-5-5-5 5M12 3v12"/>',
  },
  send: {
    strokeWidth: 2.2,
    body: '<path d="M12 19V5M6 11l6-6 6 6"/>',
  },
  message: {
    strokeWidth: 1.75,
    body: '<path d="M21 12a8 8 0 0 1-11.5 7.2L4 21l1.8-5.5A8 8 0 1 1 21 12z"/>',
  },
  // lucide terminal (chevron + baseline) — prompt actions in the `/` slash menu
  prompt: {
    strokeWidth: 2,
    body: '<path d="M12 19h8"/><path d="m4 17 6-6-6-6"/>',
  },
  activity: {
    strokeWidth: 1.75,
    body: '<path d="M3 12h4l3-8 4 16 3-8h4"/>',
  },
  house: {
    // lucide house — the project's main working copy (real checkout)
    strokeWidth: 1.9,
    body: '<path d="M15 21v-8a1 1 0 0 0-1-1h-4a1 1 0 0 0-1 1v8"/><path d="M3 10a2 2 0 0 1 .709-1.528l7-6a2 2 0 0 1 2.582 0l7 6A2 2 0 0 1 21 10v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>',
  },
  'house-plus': {
    // lucide house-plus — Add project (distinct from per-project New workspace +)
    strokeWidth: 1.9,
    body:
      '<path d="M12.35 21H5a2 2 0 0 1-2-2v-9a2 2 0 0 1 .71-1.53l7-6a2 2 0 0 1 2.58 0l7 6A2 2 0 0 1 21 10v2.35"/>' +
      '<path d="M14.8 12.4A1 1 0 0 0 14 12h-4a1 1 0 0 0-1 1v8"/>' +
      '<path d="M15 18h6"/>' +
      '<path d="M18 15v6"/>',
  },
  folder: {
    body: '<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>',
  },
  'folder-simple': {
    strokeWidth: 1.75,
    body: '<path d="M4 6h6l2 2h8v10H4z"/>',
  },
  // lucide workflow — file-list tree mode
  workflow: {
    body:
      '<rect width="8" height="8" x="3" y="3" rx="2"/>' +
      '<path d="M7 11v4a2 2 0 0 0 2 2h4"/>' +
      '<rect width="8" height="8" x="13" y="13" rx="2"/>',
  },
  file: {
    body:
      '<path d="M6 2h8l5 5v13a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2z"/>' +
      '<path d="M14 2v5h5"/>',
  },
  'file-diff': {
    // lucide file-diff — Files changed tab
    body:
      '<path d="M6 22a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.704.706l3.588 3.588A2.4 2.4 0 0 1 20 8v12a2 2 0 0 1-2 2z"/>' +
      '<path d="M9 10h6"/><path d="M12 13V7"/><path d="M9 17h6"/>',
  },
  // tabler sitemap — mermaid artifacts
  sitemap: {
    strokeWidth: 2,
    body:
      '<path d="M3 17a2 2 0 0 1 2 -2h2a2 2 0 0 1 2 2v2a2 2 0 0 1 -2 2h-2a2 2 0 0 1 -2 -2l0 -2"/>' +
      '<path d="M15 17a2 2 0 0 1 2 -2h2a2 2 0 0 1 2 2v2a2 2 0 0 1 -2 2h-2a2 2 0 0 1 -2 -2l0 -2"/>' +
      '<path d="M9 5a2 2 0 0 1 2 -2h2a2 2 0 0 1 2 2v2a2 2 0 0 1 -2 2h-2a2 2 0 0 1 -2 -2l0 -2"/>' +
      '<path d="M6 15v-1a2 2 0 0 1 2 -2h8a2 2 0 0 1 2 2v1"/>' +
      '<path d="M12 9l0 3"/>',
  },
  // tabler file-type-html
  'file-type-html': {
    strokeWidth: 2,
    body:
      '<path d="M14 3v4a1 1 0 0 0 1 1h4"/>' +
      '<path d="M5 12v-7a2 2 0 0 1 2 -2h7l5 5v4"/>' +
      '<path d="M2 21v-6"/>' +
      '<path d="M5 15v6"/>' +
      '<path d="M2 18h3"/>' +
      '<path d="M20 15v6h2"/>' +
      '<path d="M13 21v-6l2 3l2 -3v6"/>' +
      '<path d="M7.5 15h3"/>' +
      '<path d="M9 15v6"/>',
  },
  // tabler file-type-svg
  'file-type-svg': {
    strokeWidth: 2,
    body:
      '<path d="M14 3v4a1 1 0 0 0 1 1h4"/>' +
      '<path d="M5 12v-7a2 2 0 0 1 2 -2h7l5 5v4"/>' +
      '<path d="M4 20.25c0 .414 .336 .75 .75 .75h1.25a1 1 0 0 0 1 -1v-1a1 1 0 0 0 -1 -1h-1a1 1 0 0 1 -1 -1v-1a1 1 0 0 1 1 -1h1.25a.75 .75 0 0 1 .75 .75"/>' +
      '<path d="M10 15l2 6l2 -6"/>' +
      '<path d="M20 15h-1a2 2 0 0 0 -2 2v2a2 2 0 0 0 2 2h1v-3"/>',
  },
  // tabler photo — png/jpeg artifacts
  photo: {
    strokeWidth: 2,
    body:
      '<path d="M15 8h.01"/>' +
      '<path d="M3 6a3 3 0 0 1 3 -3h12a3 3 0 0 1 3 3v12a3 3 0 0 1 -3 3h-12a3 3 0 0 1 -3 -3v-12"/>' +
      '<path d="M3 16l5 -5c.928 -.893 2.072 -.893 3 0l5 5"/>' +
      '<path d="M14 14l1 -1c.928 -.893 2.072 -.893 3 0l3 3"/>',
  },
  // tabler file-type-csv
  'file-type-csv': {
    strokeWidth: 2,
    body:
      '<path d="M14 3v4a1 1 0 0 0 1 1h4"/>' +
      '<path d="M5 12v-7a2 2 0 0 1 2 -2h7l5 5v4"/>' +
      '<path d="M7 16.5a1.5 1.5 0 0 0 -3 0v3a1.5 1.5 0 0 0 3 0"/>' +
      '<path d="M10 20.25c0 .414 .336 .75 .75 .75h1.25a1 1 0 0 0 1 -1v-1a1 1 0 0 0 -1 -1h-1a1 1 0 0 1 -1 -1v-1a1 1 0 0 1 1 -1h1.25a.75 .75 0 0 1 .75 .75"/>' +
      '<path d="M16 15l2 6l2 -6"/>',
  },
  // tabler braces — json artifacts
  braces: {
    strokeWidth: 2,
    body:
      '<path d="M7 4a2 2 0 0 0 -2 2v3a2 3 0 0 1 -2 3a2 3 0 0 1 2 3v3a2 2 0 0 0 2 2"/>' +
      '<path d="M17 4a2 2 0 0 1 2 2v3a2 3 0 0 0 2 3a2 3 0 0 0 -2 3v3a2 2 0 0 1 -2 2"/>',
  },
  // tabler markdown — md artifacts
  markdown: {
    strokeWidth: 2,
    body:
      '<path d="M3 7a2 2 0 0 1 2 -2h14a2 2 0 0 1 2 2v10a2 2 0 0 1 -2 2h-14a2 2 0 0 1 -2 -2v-10"/>' +
      '<path d="M7 15v-6l2 2l2 -2v6"/>' +
      '<path d="M14 13l2 2l2 -2m-2 2v-6"/>',
  },
  pencil: {
    body: '<path d="M17 3l4 4L8 20l-5 1 1-5z"/>',
  },
  // lucide face-slightly-smiling — add reaction
  smile: {
    body:
      '<path d="M15 10V9"/>' +
      '<path d="M16.472 15a6 6 0 01-8.943 0"/>' +
      '<path d="M9 10V9"/>' +
      '<circle cx="12" cy="12" r="10"/>',
  },
  'thumbs-up': {
    body:
      '<path d="M15 5.88 14 10h5.83a2 2 0 0 1 1.92 2.56l-2.33 8A2 2 0 0 1 17.5 22H4a2 2 0 0 1-2-2v-8a2 2 0 0 1 2-2h2.76a2 2 0 0 0 1.79-1.11L12 2a3.13 3.13 0 0 1 3 3.88Z"/>' +
      '<path d="M7 10v12"/>',
  },
  'thumbs-down': {
    body:
      '<path d="M9 18.12 10 14H4.17a2 2 0 0 1-1.92-2.56l2.33-8A2 2 0 0 1 6.5 2H20a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2h-2.76a2 2 0 0 0-1.79 1.11L12 22a3.13 3.13 0 0 1-3-3.88Z"/>' +
      '<path d="M17 14V2"/>',
  },
  // lucide face-grinning — laugh
  'face-grinning': {
    body:
      '<path d="M15 10V9"/>' +
      '<path d="M7.084 14.302a5.12 5.12 0 009.833 0 .24.24 0 00-.235-.302H7.32a.24.24 0 00-.235.302"/>' +
      '<path d="M9 10V9"/>' +
      '<circle cx="12" cy="12" r="10"/>',
  },
  'party-popper': {
    body:
      '<path d="M5.8 11.3 2 22l10.7-3.79"/>' +
      '<path d="M4 3h.01"/>' +
      '<path d="M22 8h.01"/>' +
      '<path d="M15 2h.01"/>' +
      '<path d="M22 20h.01"/>' +
      '<path d="m22 2-2.24.75a2.9 2.9 0 0 0-1.96 3.12c.1.86-.57 1.63-1.45 1.63h-.38c-.86 0-1.6.6-1.76 1.44L14 10"/>' +
      '<path d="m22 13-.82-.33c-.86-.34-1.82.2-1.98 1.11c-.11.7-.72 1.22-1.43 1.22H17"/>' +
      '<path d="m11 2 .33.82c.34.86-.2 1.82-1.11 1.98C9.52 4.9 9 5.52 9 6.23V7"/>' +
      '<path d="M11 13c1.93 1.93 2.83 4.17 2 5-.83.83-3.07-.07-5-2-1.93-1.93-2.83-4.17-2-5 .83-.83 3.07.07 5 2Z"/>',
  },
  // lucide face-slightly-frowning — confused
  'face-slightly-frowning': {
    body:
      '<path d="M15 10V9"/>' +
      '<path d="M9 10V9"/>' +
      '<path d="M9 16a5 5 0 016 0"/>' +
      '<circle cx="12" cy="12" r="10"/>',
  },
  rocket: {
    body:
      '<path d="M12 15v5s3.03-.55 4-2c1.08-1.62 0-5 0-5"/>' +
      '<path d="M4.5 16.5c-1.5 1.26-2 5-2 5s3.74-.5 5-2c.71-.84.7-2.13-.09-2.91a2.18 2.18 0 0 0-2.91-.09"/>' +
      '<path d="M9 12a22 22 0 0 1 2-3.95A12.88 12.88 0 0 1 22 2c0 2.72-.78 7.5-6 11a22.4 22.4 0 0 1-4 2z"/>' +
      '<path d="M9 12H4s.55-3.03 2-4c1.62-1.08 5 .05 5 .05"/>',
  },
  // lucide ellipsis — comment overflow (⋯)
  ellipsis: {
    filled: true,
    body: '<circle cx="5" cy="12" r="1.7"/><circle cx="12" cy="12" r="1.7"/><circle cx="19" cy="12" r="1.7"/>',
  },
  // lucide quote
  quote: {
    body:
      '<path d="M16 3a2 2 0 0 0-2 2v6a2 2 0 0 0 2 2 1 1 0 0 1 1 1v1a2 2 0 0 1-2 2 1 1 0 0 0-1 1v2a1 1 0 0 0 1 1 6 6 0 0 0 6-6V5a2 2 0 0 0-2-2z"/>' +
      '<path d="M5 3a2 2 0 0 0-2 2v6a2 2 0 0 0 2 2 1 1 0 0 1 1 1v1a2 2 0 0 1-2 2 1 1 0 0 0-1 1v2a1 1 0 0 0 1 1 6 6 0 0 0 6-6V5a2 2 0 0 0-2-2z"/>',
  },
  wrench: {
    body:
      '<path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.106-3.105c.32-.322.863-.22.983.218a6 6 0 0 1-8.259 7.057l-7.91 7.91a1 1 0 0 1-2.999-3l7.91-7.91a6 6 0 0 1 7.057-8.259c.438.12.54.662.219.984z"/>',
  },
  globe: {
    body: '<circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18"/>',
  },
  bot: {
    body:
      '<path d="M12 8V4H8"/>' +
      '<rect width="16" height="12" x="4" y="8" rx="2"/>' +
      '<path d="M2 14h2M20 14h2M15 13v2M9 13v2"/>',
  },
  agent: {
    body: '<rect x="5" y="7" width="14" height="12" rx="2"/><path d="M12 7V4M8 12h.01M16 12h.01"/>',
  },
  'list-todo': {
    body:
      '<path d="M13 5h8M13 12h8M13 19h8"/>' +
      '<path d="m3 17 2 2 4-4"/>' +
      '<rect x="3" y="4" width="6" height="6" rx="1"/>',
  },
  lightbulb: {
    body: '<path d="M12 3a7 7 0 0 1 7 7c0 2.4-1.2 4.2-2.6 5.5-.6.6-.9 1.4-.9 2.2V19a2 2 0 0 1-2 2h-3a2 2 0 0 1-2-2v-1.3c0-.8-.3-1.6-.9-2.2C6.2 14.2 5 12.4 5 10a7 7 0 0 1 7-7z"/>',
  },
  flask: {
    body: '<path d="M9 7V4M15 7V4M7 7h10v5a5 5 0 0 1-10 0zM12 17v4"/>',
  },
  'hat-glasses': {
    body:
      '<path d="M14 18a2 2 0 0 0-4 0"/>' +
      '<path d="m19 11-2.11-6.657a2 2 0 0 0-2.752-1.148l-1.276.61A2 2 0 0 1 12 4H8.5a2 2 0 0 0-1.925 1.456L5 11"/>' +
      '<path d="M2 11h20"/>' +
      '<circle cx="17" cy="18" r="3"/>' +
      '<circle cx="7" cy="18" r="3"/>',
  },
  // lucide message-square — unresolved review comments on Files changed
  'message-square': {
    body: '<path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/>',
  },
  'pocket-knife': {
    body:
      '<path d="M3 2v1c0 1 2 1 2 2S3 6 3 7s2 1 2 2-2 1-2 2 2 1 2 2"/>' +
      '<path d="M18 6h.01"/>' +
      '<path d="M6 18h.01"/>' +
      '<path d="M20.83 8.83a4 4 0 0 0-5.66-5.66l-12 12a4 4 0 1 0 5.66 5.66Z"/>' +
      '<path d="M18 11.66V22a4 4 0 0 0 4-4V6"/>',
  },
  'bug-off': {
    body:
      '<path d="M12 20v-8"/>' +
      '<path d="M12.656 7H14a4 4 0 0 1 4 4v1.344"/>' +
      '<path d="M14.12 3.88 16 2"/>' +
      '<path d="M17.123 17.123A6 6 0 0 1 6 14v-3a4 4 0 0 1 1.72-3.287"/>' +
      '<path d="m2 2 20 20"/>' +
      '<path d="M21 5a4 4 0 0 1-3.55 3.97"/>' +
      '<path d="M22 13h-3.344"/>' +
      '<path d="M3 21a4 4 0 0 1 3.81-4"/>' +
      '<path d="M3 5a4 4 0 0 0 3.55 3.97"/>' +
      '<path d="M6 13H2"/>' +
      '<path d="m8 2 1.88 1.88"/>' +
      '<path d="M9.712 4.06A3 3 0 0 1 15 6v1.13"/>',
  },
  // Lucide "eye" / "eye-off" — reveal / hide secret fields.
  eye: {
    body:
      '<path d="M2.062 12.348a1 1 0 0 1 0-.696 10.75 10.75 0 0 1 19.876 0 1 1 0 0 1 0 .696 10.75 10.75 0 0 1-19.876 0"/>' +
      '<circle cx="12" cy="12" r="3"/>',
  },
  'eye-off': {
    body:
      '<path d="M10.733 5.076a10.744 10.744 0 0 1 11.205 6.575 1 1 0 0 1 0 .696 10.747 10.747 0 0 1-1.444 2.49"/>' +
      '<path d="M14.084 14.158a3 3 0 0 1-4.242-4.242"/>' +
      '<path d="M17.479 17.499a10.75 10.75 0 0 1-15.417-5.151 1 1 0 0 1 0-.696 10.75 10.75 0 0 1 4.446-5.143"/>' +
      '<path d="m2 2 20 20"/>',
  },
  zap: {
    body: '<path d="M13 2L4 14h6l-1 8 9-12h-6z"/>',
  },
  sparkle: {
    strokeWidth: 1.75,
    body:
      '<path d="M12 3l1.5 5.5L19 10l-5.5 1.5L12 17l-1.5-5.5L5 10l5.5-1.5L12 3z"/>' +
      '<path d="M19 15l.7 2.3L22 18l-2.3.7L19 21l-.7-2.3L16 18l2.3-.7L19 15z"/>',
  },
  // lucide palette — Settings → Bundles → Themes
  palette: {
    body:
      '<path d="M12 22a1 1 0 0 1 0-20 10 9 0 0 1 10 9 5 5 0 0 1-5 5h-2.25a1.75 1.75 0 0 0-1.4 2.8l.3.4a1.75 1.75 0 0 1-1.4 2.8z"/>' +
      '<circle cx="13.5" cy="6.5" r=".5" fill="currentColor"/>' +
      '<circle cx="17.5" cy="10.5" r=".5" fill="currentColor"/>' +
      '<circle cx="6.5" cy="12.5" r=".5" fill="currentColor"/>' +
      '<circle cx="8.5" cy="7.5" r=".5" fill="currentColor"/>',
  },
  // Lucide "sparkles" — Skill tool activation rows.
  sparkles: {
    body:
      '<path d="M11.017 2.814a1 1 0 0 1 1.966 0l1.051 5.558a2 2 0 0 0 1.594 1.594l5.558 1.051a1 1 0 0 1 0 1.966l-5.558 1.051a2 2 0 0 0-1.594 1.594l-1.051 5.558a1 1 0 0 1-1.966 0l-1.051-5.558a2 2 0 0 0-1.594-1.594l-5.558-1.051a1 1 0 0 1 0-1.966l5.558-1.051a2 2 0 0 0 1.594-1.594z"/>' +
      '<path d="M20 2v4"/>' +
      '<path d="M22 4h-4"/>' +
      '<circle cx="4" cy="20" r="2"/>',
  },
  // Slash-command affordance — steeper than Lucide's 45° so it reads like `/`.
  slash: {
    body: '<path d="M15 3.5 9 20.5"/>',
  },
  copy: {
    body: '<rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>',
  },
  heart: {
    body: '<path d="M2 9.5a5.5 5.5 0 0 1 9.591-3.676.56.56 0 0 0 .818 0A5.49 5.49 0 0 1 22 9.5c0 2.29-1.5 4-3 5.5l-5.492 5.313a2 2 0 0 1-3 .019L5 15c-1.5-1.5-3-3.2-3-5.5"/>',
  },
  list: {
    strokeWidth: 1.75,
    body: '<path d="M8 6h12M8 12h12M8 18h12M4 6h.01M4 12h.01M4 18h.01"/>',
  },
  hash: {
    // lucide hash — go-to-line dialog glyph
    body: '<path d="M4 9h16M4 15h16M10 3L8 21M16 3l-2 18"/>',
  },
  'align-left': {
    body: '<path d="M4 6h16M4 12h16M4 18h10"/>',
  },
  refs: {
    strokeWidth: 1.75,
    body:
      '<path d="M10 6H6v4M14 6h4v4M10 18H6v-4M14 18h4v-4"/>' +
      '<path d="M8 10v4M16 10v4M10 8h4M10 16h4"/>',
  },
  undo: {
    strokeWidth: 1.75,
    body:
      '<path d="M9 14 4 9l5-5"/>' +
      '<path d="M4 9h10.5a5.5 5.5 0 0 1 5.5 5.5v0a5.5 5.5 0 0 1-5.5 5.5H11"/>',
  },
  shield: {
    body: '<path d="M12 2l8 4v6c0 5-3.5 8-8 10-4.5-2-8-5-8-10V6z"/>',
  },
  'shield-check': {
    body:
      '<path d="M12 2l8 4v6c0 5-3.5 8-8 10-4.5-2-8-5-8-10V6z"/>' +
      '<path d="M9 12l2 2 4-4"/>',
  },
  clock: {
    body: '<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>',
  },
  // lucide clock-fading — one-time schedule (Repeat picker)
  'clock-fading': {
    body:
      '<path d="M12 2a10 10 0 0 1 7.38 16.75"/>' +
      '<path d="M12 6v6l4 2"/>' +
      '<path d="M2.5 8.875a10 10 0 0 0-.5 3"/>' +
      '<path d="M2.83 16a10 10 0 0 0 2.43 3.4"/>' +
      '<path d="M4.636 5.235a10 10 0 0 1 .891-.857"/>' +
      '<path d="M8.644 21.42a10 10 0 0 0 7.631-.38"/>',
  },
  // lucide rotate-cw-fading-clock — cron schedule (Repeat picker)
  'rotate-cw-fading-clock': {
    body:
      '<path d="M12 3a9.75 9.75 0 0 1 6.74 2.74"/>' +
      '<path d="M18.74 5.74 21 8"/>' +
      '<path d="M21 8V3"/>' +
      '<path d="M7.5 19.794c-6-3.464-6-12.124 0-15.588"/>' +
      '<path d="M7.5 4.206A9 9 0 0 1 12 3"/>' +
      '<path d="M12 7v5l4 2"/>' +
      '<path d="M14 20.775A9 9 0 0 1 12 21"/>' +
      '<path d="M19 17.656a9 9 0 0 1-1.5 1.456"/>' +
      '<path d="M21 12a9 9 0 0 1-.228 2"/>' +
      '<path d="M21 8h-5"/>',
  },
  // lucide clock-plus — New schedule (sidebar project row, palette)
  'clock-plus': {
    body:
      '<path d="M12 6v6l3.644 1.822"/>' +
      '<path d="M16 19h6"/>' +
      '<path d="M19 16v6"/>' +
      '<path d="M21.92 13.267a10 10 0 1 0-8.653 8.653"/>',
  },
  // lucide clock-arrow-down — PR list recently updated
  'clock-arrow-down': {
    body:
      '<path d="M12 6v6l2 1"/>' +
      '<path d="M12.337 21.994a10 10 0 1 1 9.588-8.767"/>' +
      '<path d="m14 18 4 4 4-4"/>' +
      '<path d="M18 14v8"/>',
  },
  // lucide clock-arrow-up — PR list least recently updated
  'clock-arrow-up': {
    body:
      '<path d="M12 6v6l1.56.78"/>' +
      '<path d="M13.227 21.925a10 10 0 1 1 8.767-9.588"/>' +
      '<path d="m14 18 4-4 4 4"/>' +
      '<path d="M18 22v-8"/>',
  },
  // lucide calendar-arrow-down — PR list newest
  'calendar-arrow-down': {
    body:
      '<path d="m14 17 4 4 4-4"/>' +
      '<path d="M16 2v3"/>' +
      '<path d="M18 13v8"/>' +
      '<path d="M21 10.354V5a2 2 0 00-2-2H5a2 2 0 00-2 2v14a2 2 0 002 2h7.343"/>' +
      '<path d="M3 9h18"/>' +
      '<path d="M8 2v3"/>',
  },
  // lucide calendar-arrow-up — PR list oldest
  'calendar-arrow-up': {
    body:
      '<path d="m14 17 4-4 4 4"/>' +
      '<path d="M16 2v3"/>' +
      '<path d="M18 21v-8"/>' +
      '<path d="M21 10.343V5a2 2 0 00-2-2H5a2 2 0 00-2 2v14a2 2 0 002 2h9"/>' +
      '<path d="M3 9h18"/>' +
      '<path d="M8 2v3"/>',
  },
  // lucide arrow-up-a-z — PR list title A–Z
  'arrow-up-az': {
    body:
      '<path d="m3 8 4-4 4 4"/>' +
      '<path d="M7 4v16"/>' +
      '<path d="M20 8h-5"/>' +
      '<path d="M15 10V6.5a2.5 2.5 0 0 1 5 0V10"/>' +
      '<path d="M15 14h5l-5 6h5"/>',
  },
  user: {
    body: '<circle cx="12" cy="8" r="3.5"/><path d="M5 20c0-3.6 3.1-6.5 7-6.5s7 2.9 7 6.5"/>',
  },
  // Lucide "plug" — provider / connection affordance (Settings → Providers).
  plug: {
    body: '<path d="M12 22v-5"/><path d="M15 8V2"/><path d="M17 8a1 1 0 0 1 1 1v4a4 4 0 0 1-4 4h-4a4 4 0 0 1-4-4V9a1 1 0 0 1 1-1z"/><path d="M9 8V2"/>',
  },
  // Lucide "cpu" — Settings → Models.
  cpu: {
    body: '<path d="M12 20v2"/><path d="M12 2v2"/><path d="M17 20v2"/><path d="M17 2v2"/><path d="M2 12h2"/><path d="M2 17h2"/><path d="M2 7h2"/><path d="M20 12h2"/><path d="M20 17h2"/><path d="M20 7h2"/><path d="M7 20v2"/><path d="M7 2v2"/><rect x="4" y="4" width="16" height="16" rx="2"/><rect x="8" y="8" width="8" height="8" rx="1"/>',
  },
  'user-plus': {
    body:
      '<circle cx="9" cy="8" r="3"/>' +
      '<path d="M3 20c0-3.3 2.7-6 6-6s6 2.7 6 6"/>' +
      '<circle cx="18" cy="9" r="2.5"/>',
  },
  gauge: {
    body:
      '<path d="M4 14a8 8 0 0 1 16 0"/>' +
      '<path d="M12 14V8"/>' +
      '<circle cx="12" cy="16" r="1.5" fill="currentColor" stroke="none"/>',
  },
  monitor: {
    body:
      '<rect width="20" height="14" x="2" y="3" rx="2"/>' +
      '<line x1="8" x2="16" y1="21" y2="21"/>' +
      '<line x1="12" x2="12" y1="17" y2="21"/>',
  },
  code: {
    body: '<path d="m18 16 4-4-4-4"/><path d="m6 8-4 4 4 4"/><path d="m14.5 4-5 16"/>',
  },
  // lucide blocks — Settings → Bundles
  blocks: {
    body:
      '<path d="M10 22V7a1 1 0 0 0-1-1H4a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-5a1 1 0 0 0-1-1H2"/>' +
      '<rect x="14" y="2" width="8" height="8" rx="1"/>',
  },
  trash: {
    body: '<path d="M5 7h14M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4h6v3"/>',
  },
  'file-plus': {
    viewBox: '0 0 256 256',
    strokeWidth: 16,
    body:
      '<path d="M200,224H56a8,8,0,0,1-8-8V40a8,8,0,0,1,8-8h96l56,56V216A8,8,0,0,1,200,224Z"/>' +
      '<polyline points="152 32 152 88 208 88"/>' +
      '<line x1="104" y1="152" x2="152" y2="152"/>' +
      '<line x1="128" y1="128" x2="128" y2="176"/>',
  },
  'folder-plus': {
    viewBox: '0 0 256 256',
    strokeWidth: 16,
    body:
      '<path d="M224,88V200.89a7.11,7.11,0,0,1-7.11,7.11H40a8,8,0,0,1-8-8V64a8,8,0,0,1,8-8H93.33a8,8,0,0,1,4.8,1.6L128,80h88A8,8,0,0,1,224,88Z"/>' +
      '<line x1="104" y1="144" x2="152" y2="144"/>' +
      '<line x1="128" y1="120" x2="128" y2="168"/>',
  },
  // ---- chat-mode identity (lucide) ----
  'message-question': {
    // lucide message-circle-question-mark — Ask mode
    body:
      '<path d="M2.992 16.342a2 2 0 0 1 .094 1.167l-1.065 3.29a1 1 0 0 0 1.236 1.168l3.413-.998a2 2 0 0 1 1.099.092 10 10 0 1 0-4.777-4.719"/>' +
      '<path d="M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3"/>' +
      '<path d="M12 17h.01"/>',
  },
  notebook: {
    // lucide notebook — Plan mode
    body:
      '<path d="M2 6h4"/><path d="M2 10h4"/><path d="M2 14h4"/><path d="M2 18h4"/>' +
      '<rect width="16" height="20" x="4" y="2" rx="2"/>' +
      '<path d="M16 2v20"/>',
  },
  'notebook-pen': {
    // lucide notebook-pen — CreatePlan / "Wrote plan" tool card
    body:
      '<path d="M13.4 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-7.4"/>' +
      '<path d="M2 6h4"/><path d="M2 10h4"/><path d="M2 14h4"/><path d="M2 18h4"/>' +
      '<path d="M21.378 5.626a1 1 0 1 0-3.004-3.004l-5.01 5.012a2 2 0 0 0-.506.854l-.837 2.87a.5.5 0 0 0 .62.62l2.87-.837a2 2 0 0 0 .854-.506z"/>',
  },
  'message-code': {
    // lucide message-circle-code — Agent mode
    body:
      '<path d="m10 9-3 3 3 3"/><path d="m14 15 3-3-3-3"/>' +
      '<path d="M2.992 16.342a2 2 0 0 1 .094 1.167l-1.065 3.29a1 1 0 0 0 1.236 1.168l3.413-.998a2 2 0 0 1 1.099.092 10 10 0 1 0-4.777-4.719"/>',
  },
  'git-compare': {
    // lucide git-compare-arrows — Review mode
    body:
      '<circle cx="5" cy="6" r="3"/><path d="M12 6h5a2 2 0 0 1 2 2v7"/><path d="m15 9-3-3 3-3"/>' +
      '<circle cx="19" cy="18" r="3"/><path d="M12 18H7a2 2 0 0 1-2-2V9"/><path d="m9 15 3 3-3 3"/>',
  },
  // Findings severity — Tabler (filled octagon / triangle / info; outline triangle / circle).
  'sev-critical': {
    filled: true,
    body: '<path d="M15.3 2h-6.6c-.562 0 -1.016 .201 -1.407 .593l-4.7 4.7a1.894 1.894 0 0 0 -.593 1.407v6.6c0 .562 .201 1.016 .593 1.407l4.7 4.7c.391 .392 .845 .593 1.407 .593h6.6c.562 0 1.016 -.201 1.407 -.593l4.7 -4.7c.392 -.391 .593 -.845 .593 -1.407v-6.6c0 -.562 -.201 -1.016 -.593 -1.407l-4.7 -4.7a1.894 1.894 0 0 0 -1.407 -.593z"/>',
  },
  'sev-high': {
    filled: true,
    body:
      '<path d="M12 1.67c.955 0 1.845 .467 2.39 1.247l.105 .16l8.114 13.548a2.914 2.914 0 0 1 -2.307 4.363l-.195 .008h-16.225a2.914 2.914 0 0 1 -2.582 -4.2l.099 -.185l8.11 -13.538a2.914 2.914 0 0 1 2.491 -1.403zm.01 13.33l-.127 .007a1 1 0 0 0 0 1.986l.117 .007l.127 -.007a1 1 0 0 0 0 -1.986l-.117 -.007zm-.01 -7a1 1 0 0 0 -.993 .883l-.007 .117v4l.007 .117a1 1 0 0 0 1.986 0l.007 -.117v-4l-.007 -.117a1 1 0 0 0 -.993 -.883z"/>',
  },
  'sev-medium': {
    strokeWidth: 2,
    body:
      '<path d="M12 9v4"/>' +
      '<path d="M10.363 3.591l-8.106 13.534a1.914 1.914 0 0 0 1.636 2.871h16.214a1.914 1.914 0 0 0 1.636 -2.87l-8.106 -13.536a1.914 1.914 0 0 0 -3.274 0"/>' +
      '<path d="M12 16h.01"/>',
  },
  'sev-low': {
    strokeWidth: 2,
    body:
      '<path d="M3 12a9 9 0 1 0 18 0a9 9 0 0 0 -18 0"/>' +
      '<path d="M12 8v4"/>' +
      '<path d="M12 16h.01"/>',
  },
  'sev-info': {
    filled: true,
    body:
      '<path d="M12 2c5.523 0 10 4.477 10 10a10 10 0 0 1 -19.995 .324l-.005 -.324l.004 -.28c.148 -5.393 4.566 -9.72 9.996 -9.72zm0 9h-1l-.117 .007a1 1 0 0 0 0 1.986l.117 .007v3l.007 .117a1 1 0 0 0 .876 .876l.117 .007h1l.117 -.007a1 1 0 0 0 .876 -.876l.007 -.117l-.007 -.117a1 1 0 0 0 -.764 -.857l-.112 -.02l-.117 -.006v-3l-.007 -.117a1 1 0 0 0 -.876 -.876l-.117 -.007zm.01 -3l-.127 .007a1 1 0 0 0 0 1.986l.117 .007l.127 -.007a1 1 0 0 0 0 -1.986l-.117 -.007z"/>',
  },
} as const satisfies Record<string, IconDef>

export type IconName = keyof typeof icons
