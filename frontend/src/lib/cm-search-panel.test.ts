/** @vitest-environment happy-dom */
import { describe, expect, it } from 'vitest'
import { EditorState } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import { SearchQuery, openSearchPanel, getSearchQuery, setSearchQuery } from '@codemirror/search'
import { createRecurSearchPanel, recurSearchExtension } from './cm-search-panel'

function mount(doc = 'alpha beta alpha\nalpha') {
  const parent = document.createElement('div')
  document.body.appendChild(parent)
  const view = new EditorView({
    parent,
    state: EditorState.create({
      doc,
      extensions: [recurSearchExtension()],
    }),
  })
  return { view, parent }
}

describe('recur search panel', () => {
  it('opens a Recur panel with main-field instead of stock cm-search', () => {
    const { view, parent } = mount()
    openSearchPanel(view)
    const panel = parent.querySelector('.cm-recur-search')
    expect(panel).toBeTruthy()
    expect(parent.querySelector('.cm-search.cm-panel')).toBeNull()
    expect(panel!.querySelector('[main-field]')).toBeTruthy()
    expect(panel!.querySelector('.cm-recur-search-toggle')).toBeTruthy()
    view.destroy()
    parent.remove()
  })

  it('commits toggle + query into search state', () => {
    const { view, parent } = mount()
    const panel = createRecurSearchPanel(view)
    parent.appendChild(panel.dom)
    const input = panel.dom.querySelector('[name=search]') as HTMLInputElement
    const caseBtn = panel.dom.querySelector('.cm-recur-search-toggle') as HTMLButtonElement
    input.value = 'alpha'
    input.dispatchEvent(new Event('input'))
    caseBtn.click()
    const q = getSearchQuery(view.state)
    expect(q.search).toBe('alpha')
    expect(q.caseSensitive).toBe(true)
    view.destroy()
    parent.remove()
  })

  it('shows match count for the current query', () => {
    const { view, parent } = mount()
    view.dispatch({
      effects: setSearchQuery.of(new SearchQuery({ search: 'alpha' })),
    })
    const panel = createRecurSearchPanel(view)
    parent.appendChild(panel.dom)
    panel.mount?.()
    const count = panel.dom.querySelector('.cm-recur-search-count')
    expect(count?.textContent).toMatch(/\/ 3/)
    view.destroy()
    parent.remove()
  })
})
