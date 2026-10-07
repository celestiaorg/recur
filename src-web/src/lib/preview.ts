/** State posted to the live app preview iframe (ThemePreview). */

/** Which surface the mock window is showing. */
export type Scene = 'workspace' | 'loop' | 'review' | 'inbox' | 'schedules'
/** Which chat tab is lit inside the auth-refresh workspace. */
export type ChatTab = 'agent' | 'review' | 'ask'
/** What the workspace main column shows. */
export type MainView = 'editor' | 'changes'

export type PreviewView = {
  scene: Scene
  tab: ChatTab
  main: MainView
  palette: boolean
}

export const defaultView: PreviewView = {
  scene: 'workspace',
  tab: 'agent',
  main: 'editor',
  palette: false,
}
