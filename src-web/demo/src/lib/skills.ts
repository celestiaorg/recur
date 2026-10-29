/** Shared helpers for Settings → Skills and Project → Skills. */

export type SkillRow = {
  name: string
  description: string
  scope: string
  path: string
}

export type SkillKind = 'recur' | 'project' | 'user'

export function isRecurSkillPath(path: string): boolean {
  const p = path.replace(/\\/g, '/')
  return p.includes('/.recur/recur-skills/')
}

export function skillKind(s: SkillRow): SkillKind {
  if (isRecurSkillPath(s.path)) return 'recur'
  if (s.scope === 'workspace') return 'project'
  return 'user'
}

/**
 * Skill-root path markers. Several also appear inside repos (e.g. `.claude/skills`),
 * so they must not be used to infer "home" for workspace-scope skills.
 */
const SKILL_ROOT_MARKERS = [
  '/.recur/recur-skills/',
  '/.claude/skills/',
  '/.agents/skills/',
  '/.cursor/skills/',
  '/.codex/skills/',
  '/.gemini/skills/',
]

/** Home-only marker — never used as a workspace skill root. */
const HOME_ONLY_MARKERS = ['/.recur/recur-skills/']

/** Infer the machine home dir from a user/recur skill path (works on Windows + Unix). */
export function inferHomeFromSkillPath(path: string): string {
  const p = path.replace(/\\/g, '/')
  // Prefer home-only markers; fall back to shared markers only when the path
  // is clearly under a Unix/Windows home directory shape.
  for (const m of HOME_ONLY_MARKERS) {
    const i = p.indexOf(m)
    if (i > 0) return p.slice(0, i)
  }
  for (const m of SKILL_ROOT_MARKERS) {
    const i = p.indexOf(m)
    if (i <= 0) continue
    const prefix = p.slice(0, i)
    if (/^(?:\/Users\/[^/]+|\/home\/[^/]+|[A-Za-z]:\/Users\/[^/]+)$/.test(prefix)) {
      return prefix
    }
  }
  return ''
}

/**
 * Directory containing SKILL.md for display.
 * Workspace skills → repo-relative (`.claude/skills/foo`); user/recur → `~` short form.
 */
export function skillLocation(path: string, scope?: string, homeDir?: string): string {
  if (!path) return ''
  let dir = path.replace(/\\/g, '/')
  dir = dir.replace(/\/SKILL\.md$/i, '').replace(/\/skill\.md$/i, '')

  if (scope === 'workspace') {
    for (const m of SKILL_ROOT_MARKERS) {
      const i = dir.indexOf(m)
      if (i >= 0) return dir.slice(i + 1) // drop leading slash → `.claude/skills/foo`
    }
    return dir
  }

  const home = (homeDir || inferHomeFromSkillPath(path)).replace(/\/$/, '')
  if (home && (dir === home || dir.startsWith(home + '/'))) {
    return '~' + dir.slice(home.length)
  }
  return dir
}

export function mapSkillRows(
  list: { name?: string; description?: string; scope?: string; path?: string }[] | null | undefined,
): SkillRow[] {
  return (list ?? []).map((s) => ({
    name: s.name || '',
    description: s.description || '',
    scope: s.scope || 'user',
    path: s.path || '',
  }))
}

export function nameListed(list: string[] | undefined | null, name: string): boolean {
  return !!list?.includes(name)
}

/** Effective on/off for a skill given global + project override lists. */
export function effectiveSkillEnabled(
  name: string,
  globalDisabled: string[] | undefined | null,
  projectDisabled: string[] | undefined | null,
  projectEnabled: string[] | undefined | null,
): boolean {
  if (nameListed(projectDisabled, name)) return false
  if (nameListed(projectEnabled, name)) return true
  return !nameListed(globalDisabled, name)
}

export function toggleDisabledName(cur: string[] | undefined, name: string, enabled: boolean): string[] {
  const list = cur ?? []
  if (enabled) return list.filter((x) => x !== name)
  return list.includes(name) ? list : [...list, name]
}

/**
 * Adjust project EnabledSkills / DisabledSkills so the skill's effective state
 * becomes wantEnabled, storing only overrides vs the global default.
 */
export function projectSkillOverrideLists(
  name: string,
  wantEnabled: boolean,
  globalDisabled: string[] | undefined | null,
  projectDisabled: string[] | undefined | null,
  projectEnabled: string[] | undefined | null,
): { disabledSkills: string[]; enabledSkills: string[] } {
  const globalOn = !nameListed(globalDisabled, name)
  let disabled = [...(projectDisabled ?? [])]
  let enabled = [...(projectEnabled ?? [])]
  const drop = (arr: string[]) => arr.filter((x) => x !== name)
  const add = (arr: string[]) => (arr.includes(name) ? arr : [...arr, name])

  if (wantEnabled) {
    disabled = drop(disabled)
    enabled = globalOn ? drop(enabled) : add(enabled)
  } else {
    enabled = drop(enabled)
    disabled = globalOn ? add(disabled) : drop(disabled)
  }
  return { disabledSkills: disabled, enabledSkills: enabled }
}

export function skillOverrideHint(
  name: string,
  globalDisabled: string[] | undefined | null,
  projectDisabled: string[] | undefined | null,
  projectEnabled: string[] | undefined | null,
): string {
  const globalOn = !nameListed(globalDisabled, name)
  const on = effectiveSkillEnabled(name, globalDisabled, projectDisabled, projectEnabled)
  if (on && !globalOn) return 'On for this project (off globally)'
  if (!on && globalOn) return 'Off for this project'
  return ''
}

export function skillKindOverrideCount(
  skills: SkillRow[],
  projectDisabled: string[] | undefined | null,
  projectEnabled: string[] | undefined | null,
): number {
  const names = new Set(skills.map((s) => s.name))
  let n = 0
  for (const name of projectDisabled ?? []) if (names.has(name)) n++
  for (const name of projectEnabled ?? []) if (names.has(name)) n++
  return n
}

export function skillKindHasOverrides(
  skills: SkillRow[],
  kind: SkillKind,
  projectDisabled: string[] | undefined | null,
  projectEnabled: string[] | undefined | null,
): boolean {
  const ofKind = skills.filter((s) => skillKind(s) === kind)
  return skillKindOverrideCount(ofKind, projectDisabled, projectEnabled) > 0
}
