/** Static keyboard-shortcut cheatsheet (⌘⇧/ overlay). Display glyphs match Kbd.svelte. */

export type ShortcutEntry = {
  group: string
  label: string
  keys: string
  /** Extra search terms beyond label/group. */
  terms?: string[]
}

export const SHORTCUT_GROUPS = [
  'General',
  'Navigation',
  'Workbench',
  'Editor',
  'Explorer',
  'Terminal',
] as const

export const SHORTCUTS: ShortcutEntry[] = [
  // General
  { group: 'General', label: 'Keyboard shortcuts', keys: '⌘⇧/' },
  { group: 'General', label: 'Go to (workspaces / PRs)', keys: '⌘K', terms: ['command palette', 'search'] },
  { group: 'General', label: 'Run command / snippets', keys: '⌘⇧P', terms: ['bundles', 'command palette', 'snippets', 'tab trigger'] },
  { group: 'General', label: 'New workspace', keys: '⌘N' },
  { group: 'General', label: 'New blank workspace', keys: '⇧⌘N', terms: ['quick create', 'empty'] },
  { group: 'General', label: 'Open file or folder', keys: '⌥⌘O', terms: ['open', 'folder', 'file'] },
  { group: 'General', label: 'Open in…', keys: '⌘O', terms: ['external editor', 'finder', 'terminal'] },
  { group: 'General', label: 'Toggle sidebar', keys: '⌘B' },
  { group: 'General', label: 'Settings', keys: '⌘,' },
  { group: 'General', label: 'Back', keys: 'esc', terms: ['escape'] },

  // Navigation
  { group: 'Navigation', label: 'Quick Open', keys: '⌘P' },
  { group: 'Navigation', label: 'Go to symbol', keys: '⌘⇧O' },
  { group: 'Navigation', label: 'Focus agent panel', keys: '⌘1' },
  { group: 'Navigation', label: 'Focus editor', keys: '⌘2' },
  { group: 'Navigation', label: 'Focus terminal', keys: '⌘3' },
  { group: 'Navigation', label: 'Add file / selection to chat', keys: '⌘L', terms: ['@ file', 'mention', 'reference'] },
  { group: 'Navigation', label: 'Previous tab', keys: '⌥⌘←' },
  { group: 'Navigation', label: 'Next tab', keys: '⌥⌘→' },
  { group: 'Navigation', label: 'Previous workspace', keys: '⌥⌘↑', terms: ['cycle'] },
  { group: 'Navigation', label: 'Next workspace', keys: '⌥⌘↓', terms: ['cycle'] },

  // Workbench
  { group: 'Workbench', label: 'Toggle left panel', keys: '⌥⌘B' },
  { group: 'Workbench', label: 'Toggle right panel', keys: '⌥⌘⇧B' },
  { group: 'Workbench', label: 'Toggle editor', keys: '⌥⌘E', terms: ['hide editor', 'show editor'] },
  { group: 'Workbench', label: 'Toggle review', keys: '⌥⌘R', terms: ['files changed', 'commits', 'diff'] },
  { group: 'Workbench', label: 'Review: next / previous file', keys: 'J / K', terms: ['diff', 'review'] },
  { group: 'Workbench', label: 'Open diff line in editor', keys: '⌘Click', terms: ['review', 'diff', 'jump'] },
  { group: 'Workbench', label: 'Toggle terminal', keys: '⌃`' },
  { group: 'Workbench', label: 'Maximize terminal', keys: '⌃⇧`' },
  { group: 'Workbench', label: 'New tab', keys: '⌘T', terms: ['terminal', 'agent'] },
  { group: 'Workbench', label: 'Close tab', keys: '⌘W' },
  { group: 'Workbench', label: 'Reopen closed editor tab', keys: '⌘⇧T' },
  { group: 'Workbench', label: 'Stop agent', keys: '⌘⇧⌫', terms: ['cancel', 'interrupt'] },
  { group: 'Workbench', label: 'Cycle agent mode', keys: '⌘.', terms: ['chat mode', 'ask', 'plan', 'review', 'loop'] },
  { group: 'Workbench', label: 'Split editor', keys: '⌘\\' },
  { group: 'Workbench', label: 'Run…', keys: '⌘R', terms: ['dev server', 'run command', 'run menu'] },
  { group: 'Workbench', label: 'Stop run', keys: '⌘⇧R', terms: ['dev server', 'interrupt'] },
  { group: 'Workbench', label: 'Reload from disk', keys: 'F5' },
  { group: 'Workbench', label: 'Zoom in', keys: '⌘+', terms: ['bigger', 'larger', 'scale', 'ui size'] },
  { group: 'Workbench', label: 'Zoom out', keys: '⌘-', terms: ['smaller', 'scale', 'ui size'] },
  { group: 'Workbench', label: 'Actual size', keys: '⌘0', terms: ['reset zoom', 'scale', 'ui size'] },
  { group: 'Workbench', label: 'Zoom with mouse wheel', keys: '⌘Scroll', terms: ['ctrl', 'scale', 'ui size'] },

  // Editor
  { group: 'Editor', label: 'Cycle view', keys: '⌘⇧V', terms: ['code', 'diff', 'preview', 'toggle'] },
  { group: 'Editor', label: 'Previous / next editor theme', keys: '⌥⌘[ ⌥⌘]', terms: ['cycle', 'color scheme'] },
  { group: 'Editor', label: 'Save', keys: '⌘S' },
  { group: 'Editor', label: 'Find', keys: '⌘F' },
  { group: 'Editor', label: 'Find next / previous', keys: '⌘G ⇧⌘G', terms: ['Enter', 'F3', 'shift Enter', 'shift F3'] },
  { group: 'Editor', label: 'Go to line', keys: '⌥⌘G' },
  { group: 'Editor', label: 'Revert diff hunk', keys: '⌥⌘Z', terms: ['diff', 'reject', 'undo hunk'] },
  { group: 'Editor', label: 'Match case (find)', keys: '⌥⌘C', terms: ['search panel'] },
  { group: 'Editor', label: 'Regexp (find)', keys: '⌥⌘R', terms: ['search panel', 'regex'] },
  { group: 'Editor', label: 'Whole word (find)', keys: '⌥⌘W', terms: ['search panel'] },
  { group: 'Editor', label: 'Select next occurrence', keys: '⌘D' },
  { group: 'Editor', label: 'Add cursor above / below', keys: '⌃⇧↑ ⌃⇧↓', terms: ['multi cursor'] },
  { group: 'Editor', label: 'Toggle comment', keys: '⌘/' },
  { group: 'Editor', label: 'Go to definition', keys: 'F12', terms: ['⌘-click'] },

  // Explorer (Files pane focused)
  { group: 'Explorer', label: 'Open file', keys: '⌘↓' },
  { group: 'Explorer', label: 'New file', keys: '⌥⌘N' },
  { group: 'Explorer', label: 'New folder', keys: '⇧⌘N' },
  { group: 'Explorer', label: 'Rename', keys: '↩', terms: ['enter', 'F2'] },
  { group: 'Explorer', label: 'Delete', keys: '⌘⌫' },
  { group: 'Explorer', label: 'Cut / Copy / Paste', keys: '⌘X ⌘C ⌘V' },
  { group: 'Explorer', label: 'Duplicate', keys: '⌘D' },
  { group: 'Explorer', label: 'Copy path', keys: '⌥⌘C' },
  { group: 'Explorer', label: 'Copy relative path', keys: '⇧⌥⌘C' },
  { group: 'Explorer', label: 'Reveal in Finder', keys: '⌥⌘R' },

  // Terminal (dock focused)
  { group: 'Terminal', label: 'New terminal tab', keys: '⌘T' },
  { group: 'Terminal', label: 'Close terminal tab', keys: '⌘W' },
  { group: 'Terminal', label: 'Previous / next tab', keys: '⌥⌘← ⌥⌘→' },
]
