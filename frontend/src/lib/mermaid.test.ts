/** @vitest-environment happy-dom */
import { describe, expect, test } from 'vitest'
import { unclipMermaidSvg } from './mermaid'

describe('unclipMermaidSvg', () => {
  test('sets overflow visible on the svg and label groups', () => {
    const root = document.createElement('div')
    root.innerHTML = `
      <svg overflow="hidden">
        <g class="node" overflow="hidden"><text>Hello</text></g>
        <foreignObject width="40" height="12"><div class="nodeLabel">Hi</div></foreignObject>
      </svg>`
    unclipMermaidSvg(root)
    const svg = root.querySelector('svg')!
    expect(svg.getAttribute('overflow')).toBe('visible')
    expect(root.querySelector('.node')!.getAttribute('overflow')).toBe('visible')
    expect(root.querySelector('foreignObject')!.getAttribute('overflow')).toBe('visible')
  })
})
