import type { Page } from '@playwright/test'

/** Main-pane Code surface — excludes Diff/Preview (`off`) and the split pane. */
const MAIN_CODE = '#v-detail .obs-editor-pane:not(.split-side) .obs-surface:not(.off)'
const SPLIT_CODE = '#v-detail .obs-editor-pane.split-side .obs-surface:not(.off)'

export function codeEditor(page: Page) {
  return page.locator(`${MAIN_CODE} .cm-editor`)
}

export function codeContent(page: Page) {
  return page.locator(`${MAIN_CODE} .cm-content`)
}

export function splitContent(page: Page) {
  return page.locator(`${SPLIT_CODE} .cm-content`)
}

export function diffContent(page: Page) {
  return page.locator('#v-detail .obs-diff .cm-content')
}

