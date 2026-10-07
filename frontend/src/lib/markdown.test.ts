/** @vitest-environment happy-dom */
import { afterEach, describe, expect, test } from 'vitest'
import { parseMarkdown } from './markdown-parse'
import { renderMarkdown } from './markdown'
import {
  CACHE_ENTRY_MAX_BYTES,
  clearMarkdownCache,
  markdownCacheStatsForTests,
  peekMarkdownCache,
  rememberMarkdown,
  renderMarkdownAsync,
  renderMarkdownCached,
  setMarkdownCacheMaxBytesForTests,
} from './markdown-host'

describe('renderMarkdown links', () => {
  test('keeps short http(s) labels as anchors', () => {
    const html = renderMarkdown('See [the PR](https://github.com/org/repo/pull/1).')
    expect(html).toContain('href="https://github.com/org/repo/pull/1"')
    expect(html).toContain('the PR')
  })

  test('expands GitHub emoji shortcodes outside code', () => {
    const html = renderMarkdown('Ship it :rocket: and `:smile:`')
    expect(html).toContain('🚀')
    expect(html).toContain(':smile:')
    expect(html).not.toContain(':rocket:')
  })

  test('does not turn relative paths into anchors', () => {
    const html = renderMarkdown('See [file.go](internal/file.go).')
    expect(html).not.toContain('<a ')
    expect(html).toContain('file.go')
  })

  test('highlights fenced code and strips raw script tags', () => {
    const html = renderMarkdown('```js\nconst x = 1\n```\n<script>alert(1)</script>')
    expect(html).toContain('sh-kw')
    expect(html).not.toContain('<script')
  })
})

describe('parseMarkdown', () => {
  test('does not sanitize — host / renderMarkdown must', () => {
    const raw = parseMarkdown('```js\nconst x = 1\n```')
    expect(raw).toContain('sh-kw')
  })
})

describe('renderMarkdown demoteImages', () => {
  const pixel = 'https://tracker.example/pixel.png'
  const opts = { demoteImages: true } as const

  test('does not emit a live img for ordinary, titled, or reference-style images', () => {
    const forms = [
      `![](${pixel})`,
      `![CI](${pixel} "build")`,
      `![CI](${pixel} 'build')`,
      `![CI](${pixel} (build))`,
      `![CI [main]](${pixel})`,
      `![shield][badge]\n\n[badge]: ${pixel}`,
    ]
    for (const src of forms) {
      const html = renderMarkdown(src, opts)
      expect(html, src).not.toContain('<img')
      expect(html, src).toContain(`href="${pixel}"`)
    }
  })

  test('linked badge images keep the outer href, not the badge URL', () => {
    const html = renderMarkdown(
      '[![CI](https://img.shields.io/badge.svg)](https://github.com/org/repo)',
      opts,
    )
    expect(html).not.toContain('<img')
    expect(html).toContain('href="https://github.com/org/repo"')
    expect(html).toContain('>CI<')
    expect(html).not.toContain('img.shields.io')
    expect(html.match(/<a /g)?.length).toBe(1)
  })

  test('does not rewrite image syntax inside fences or inline code', () => {
    const fenced = renderMarkdown('```md\n![alt](https://tracker.example/pixel.png)\n```', opts)
    expect(fenced).not.toContain('<img')
    expect(fenced).toContain('![alt](https://tracker.example/pixel.png)')

    const inline = renderMarkdown('Use `![alt](https://tracker.example/pixel.png)` in a README.', opts)
    expect(inline).not.toContain('<img')
    expect(inline).toContain('![alt](https://tracker.example/pixel.png)')
  })

  test('default parse still emits img so other surfaces are unchanged', () => {
    const html = renderMarkdown(`![](${pixel})`)
    expect(html).toContain('<img')
    expect(html).toContain(pixel)
  })
})

describe('renderMarkdown GitHub images', () => {
  const asset = 'https://github.com/user-attachments/assets/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee'
  const repoAsset = 'https://github.com/acme/app/assets/1/bbbbbbbb-cccc-dddd-eeee-ffffffffffff'
  const cdn = 'https://private-user-images.githubusercontent.com/1/shot.png?jwt=x'
  const pixel = 'https://tracker.example/pixel.png'

  test('proxies markdown images hosted on GitHub', () => {
    const html = renderMarkdown(`![shot](${asset})`)
    expect(html).toContain('<img')
    expect(html).toContain('/gh-image?url=')
    expect(html).toContain(encodeURIComponent(asset))
    expect(html).not.toContain(`src="${asset}"`)
  })

  test('proxies repo-assets and githubusercontent markdown images', () => {
    expect(renderMarkdown(`![](${repoAsset})`)).toContain('/gh-image?url=')
    expect(renderMarkdown(`![](${cdn})`)).toContain('/gh-image?url=')
  })

  test('keeps GitHub HTML img tags that the editor pastes', () => {
    const src = `<img width="800" alt="Screenshot" src="${asset}" />\n\nSee above.`
    const html = renderMarkdown(src)
    expect(html).toContain('<img')
    expect(html).toContain('alt="Screenshot"')
    expect(html).toContain('/gh-image?url=')
    expect(html).toContain('See above')
    expect(html).toContain('See above')
  })

  test('still strips non-GitHub HTML images and scripts', () => {
    const html = renderMarkdown(`<img src="${pixel}" alt="x"><script>alert(1)</script>`)
    expect(html).not.toContain('<img')
    expect(html).not.toContain('<script')
    expect(html).not.toContain(pixel)
  })

  test('does not treat github.com/login as an image', () => {
    const html = renderMarkdown('<img src="https://github.com/login" alt="x">')
    expect(html).not.toContain('<img')
  })

  test('does not proxy http GitHub image URLs', () => {
    const html = renderMarkdown('![](http://github.com/user-attachments/assets/x)')
    expect(html).not.toContain('/gh-image?url=')
  })

  test('does not rewrite GitHub image syntax inside fences', () => {
    const html = renderMarkdown('```md\n<img src="' + asset + '">\n```')
    expect(html).not.toContain('<img')
    expect(html).toContain('&lt;img')
  })

  test('demoteImages still turns GitHub markdown images into links', () => {
    const html = renderMarkdown(`![shot](${asset})`, { demoteImages: true })
    expect(html).not.toContain('<img')
    expect(html).toContain(`href="${asset}"`)
  })
})

describe('renderMarkdownAsync', () => {
  afterEach(() => {
    clearMarkdownCache()
  })

  test('matches sync renderMarkdown (worker or sync fallback)', async () => {
    const src = 'See [the PR](https://github.com/org/repo/pull/1).'
    await expect(renderMarkdownAsync(src)).resolves.toBe(renderMarkdown(src))
  })

  test('does not cache keyed streaming prefixes', async () => {
    const src = 'growing prefix'
    await renderMarkdownAsync(src, { key: 'stream-test' })
    expect(peekMarkdownCache(src)).toBeUndefined()
  })

  test('evicts oldest settled entries when the byte budget is exceeded', () => {
    const html = 'h'.repeat(50)
    setMarkdownCacheMaxBytesForTests(400)
    rememberMarkdown('keep', html)
    rememberMarkdown('mid', html)
    rememberMarkdown('old', html)
    rememberMarkdown('newer', html)
    expect(peekMarkdownCache('keep')).toBeUndefined()
    peekMarkdownCache('mid')
    rememberMarkdown('extra', html)
    expect(peekMarkdownCache('mid')).toBe(html)
  })

  test('skips caching a single document over the per-entry cap', () => {
    const src = 'src'
    const html = 'x'.repeat(CACHE_ENTRY_MAX_BYTES)
    rememberMarkdown(src, html)
    expect(peekMarkdownCache(src)).toBeUndefined()
    expect(markdownCacheStatsForTests().entries).toBe(0)
  })

  test('renderMarkdownCached stores a miss so the next peek hits', () => {
    const src = 'settled card'
    expect(peekMarkdownCache(src)).toBeUndefined()
    const html = renderMarkdownCached(src)
    expect(html).toBe(renderMarkdown(src))
    expect(peekMarkdownCache(src)).toBe(html)
  })

  test('demoteImages cache does not collide with the default render of the same src', () => {
    const src = '![x](https://tracker.example/pixel.png)'
    const live = renderMarkdownCached(src)
    const demoted = renderMarkdownCached(src, { demoteImages: true })
    expect(live).toContain('<img')
    expect(demoted).not.toContain('<img')
    expect(peekMarkdownCache(src)).toBe(live)
  })

  test('rememberMarkdown stores a completed document', () => {
    const src = 'done streaming'
    const html = renderMarkdown(src)
    rememberMarkdown(src, html)
    expect(peekMarkdownCache(src)).toBe(html)
  })

  test('latest same-key call resolves to its own src', async () => {
    const key = 'stream-test'
    void renderMarkdownAsync('one', { key }).catch(() => {})
    void renderMarkdownAsync('one *two*', { key }).catch(() => {})
    await expect(renderMarkdownAsync('one **two**', { key })).resolves.toBe(
      renderMarkdown('one **two**'),
    )
  })
})
