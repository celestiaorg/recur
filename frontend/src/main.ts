import './styles/app.scss'
import { mount } from 'svelte'
import App from './App.svelte'
import {
  initTheme, initEditorTheme, initEditorPrefs, initPanelWidths, initCodePanelSides,
  initExplorerPrefs, initRailCollapsed, initTerminalPrefs,
} from './lib/stores'
import { wireExternalLinks } from './lib/open-external'
import { ignoreLaunchChromeFocus } from './lib/chrome-focus'

// Apply the saved theme / panel widths before mount so there's no layout flash.
initTheme()
initEditorTheme()
initEditorPrefs()
initPanelWidths()
initCodePanelSides()
initExplorerPrefs()
initRailCollapsed()
initTerminalPrefs()

wireExternalLinks()
// Before mount so the listener is up when WKWebView focuses the first button.
ignoreLaunchChromeFocus()
mount(App, { target: document.getElementById('app')! })
