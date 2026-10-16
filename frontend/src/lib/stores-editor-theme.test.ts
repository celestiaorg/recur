import { describe, expect, it } from 'vitest'
import { get } from 'svelte/store'
import { editorTheme, setEditorThemeAuto, setSystemEditorTheme, settings, systemDark } from './stores'
import { registerThemeMeta } from './tm-registry'
import type { ThemeInfo } from '../../bindings/recur/internal/app/models'
import type { ParsedTmTheme } from './tm-theme'

const meta = (id: string) => ({ id, name: id, uiTheme: 'dark' }) as ThemeInfo

describe('editor theme sync', () => {
  it('never applies a disabled pick on a system flip', () => {
    for (const id of ['recur', 'nightowl']) registerThemeMeta(meta(id), {} as ParsedTmTheme)
    systemDark.set(false)
    setSystemEditorTheme('dark', 'nightowl')
    setEditorThemeAuto(true)
    settings.set({ disabledBundleThemes: ['nightowl'] } as never)

    systemDark.set(true)
    expect(get(editorTheme)).toBe('recur')
  })
})
