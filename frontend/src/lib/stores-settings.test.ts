import { afterEach, describe, expect, it } from 'vitest'
import { get } from 'svelte/store'
import { closeNewProject, go, newProjectOpen, openProject, openSettings, settingsSection, view } from './stores'

afterEach(() => {
  settingsSection.set('general')
  closeNewProject()
  go('inbox')
})

describe('openSettings', () => {
  it('opens Settings on the requested pane', () => {
    openSettings('skills')
    expect(get(view)).toBe('settings')
    expect(get(settingsSection)).toBe('skills')
  })

  it('leaves the current pane when none is requested', () => {
    settingsSection.set('editor')
    openSettings()
    expect(get(view)).toBe('settings')
    expect(get(settingsSection)).toBe('editor')
  })
})

describe('openProject', () => {
  it('opens a create modal instead of project settings when id is empty', () => {
    go('inbox')
    openProject('')
    expect(get(view)).toBe('briefing')
    expect(get(newProjectOpen)).toBe(true)
  })
})
