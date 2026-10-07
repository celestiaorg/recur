import { describe, expect, it } from 'vitest'
import { commandMatches, scopeForPath } from './commands'

describe('commandMatches', () => {
  it('treats unscoped commands as global', () => {
    expect(commandMatches({}, 'source.go', 'main.go')).toBe(true)
  })

  it('ORs comma-separated scopes and ANDs fileGlob', () => {
    const html = {
      scope: 'text.html, source.svelte, source.vue, source.tsx, source.js.jsx, source.astro',
      fileGlob: '*.{html,htm,svelte,vue,tsx,jsx,astro,mdx,hbs,handlebars}',
    }
    expect(commandMatches(html, 'source.svelte', 'App.svelte')).toBe(true)
    expect(commandMatches(html, 'text.html.basic', 'index.html')).toBe(true)
    expect(commandMatches(html, 'source.tsx', 'App.tsx')).toBe(true)
    expect(commandMatches(html, 'source.js.jsx', 'App.jsx')).toBe(true)
    expect(commandMatches(html, 'source.astro', 'Page.astro')).toBe(true)
    expect(commandMatches(html, 'text.html.markdown', 'page.mdx')).toBe(true)
    expect(commandMatches(html, 'source.go', 'main.go')).toBe(false)
    expect(commandMatches(html, 'source.ts', 'app.ts')).toBe(false)
    expect(commandMatches(html, 'source.js', 'app.js')).toBe(false)
    expect(commandMatches(html, 'text.html.markdown', 'readme.md')).toBe(false)
    expect(commandMatches(html, 'source.svelte', 'App.go')).toBe(false)
  })
})

describe('scopeForPath', () => {
  it('keeps tsx/jsx distinct from ts/js when no grammar is registered', () => {
    expect(scopeForPath('src/App.tsx')).toBe('source.tsx')
    expect(scopeForPath('src/App.jsx')).toBe('source.js.jsx')
    expect(scopeForPath('src/app.ts')).toBe('source.ts')
    expect(scopeForPath('src/app.js')).toBe('source.js')
  })
})

describe('commandMatches', () => {
  it('treats unscoped commands as global', () => {
    expect(commandMatches({}, 'source.go', 'main.go')).toBe(true)
  })

  it('ORs comma-separated scopes and ANDs fileGlob', () => {
    const html = {
      scope: 'text.html, source.svelte, source.vue, source.tsx, source.js.jsx, source.astro',
      fileGlob: '*.{html,htm,svelte,vue,tsx,jsx,astro,mdx,hbs,handlebars}',
    }
    expect(commandMatches(html, 'source.svelte', 'App.svelte')).toBe(true)
    expect(commandMatches(html, 'text.html.basic', 'index.html')).toBe(true)
    expect(commandMatches(html, 'source.tsx', 'App.tsx')).toBe(true)
    expect(commandMatches(html, 'source.js.jsx', 'App.jsx')).toBe(true)
    expect(commandMatches(html, 'source.astro', 'Page.astro')).toBe(true)
    expect(commandMatches(html, 'text.html.markdown', 'page.mdx')).toBe(true)
    expect(commandMatches(html, 'source.go', 'main.go')).toBe(false)
    expect(commandMatches(html, 'source.ts', 'app.ts')).toBe(false)
    expect(commandMatches(html, 'source.js', 'app.js')).toBe(false)
    expect(commandMatches(html, 'text.html.markdown', 'readme.md')).toBe(false)
    expect(commandMatches(html, 'source.svelte', 'App.go')).toBe(false)
  })
})
