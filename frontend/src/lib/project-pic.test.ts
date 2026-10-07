import { describe, expect, it } from 'vitest'
import { lucideKebab, hasLucideIcon, lucideInnerHtml, lucideNode } from './lucide-icons'
import {
  DEFAULT_LUCIDE_PIC, formatLucidePic, fgOnBackground, isProjectPresetColor,
  parseHexColor, parseProjectPic,
} from './project-pic'

describe('parseProjectPic', () => {
  it('treats a short string as a letter', () => {
    expect(parseProjectPic('S')).toEqual({ kind: 'letter', text: 'S' })
    expect(parseProjectPic('')).toEqual({ kind: 'letter', text: '?' })
  })

  it('reads lucide: names', () => {
    expect(parseProjectPic('lucide:rocket')).toEqual({ kind: 'icon', name: 'rocket' })
    expect(parseProjectPic('LUCIDE:Folder-Git')).toEqual({ kind: 'icon', name: 'folder-git' })
  })

  it('reads PNG data URLs', () => {
    const src = 'data:image/png;base64,abc'
    expect(parseProjectPic(src)).toEqual({ kind: 'image', src })
  })
})

describe('formatLucidePic', () => {
  it('prefixes a kebab name', () => {
    expect(formatLucidePic('Rocket')).toBe('lucide:rocket')
    expect(DEFAULT_LUCIDE_PIC).toBe('lucide:box')
  })
})

describe('parseHexColor', () => {
  it('normalizes 3- and 6-digit hex', () => {
    expect(parseHexColor('#4C6')).toBe('#44cc66')
    expect(parseHexColor('#4cc6d6')).toBe('#4cc6d6')
    expect(parseHexColor('var(--cyan)')).toBe('')
  })
})

describe('isProjectPresetColor', () => {
  it('recognizes the default palette only', () => {
    expect(isProjectPresetColor('var(--cyan)')).toBe(true)
    expect(isProjectPresetColor('#4cc6d6')).toBe(false)
  })
})

describe('fgOnBackground', () => {
  it('keeps dark ink on the default palette', () => {
    expect(fgOnBackground('var(--cyan)')).toBe('#0A0C10')
    expect(fgOnBackground('var(--amber)')).toBe('#0A0C10')
    expect(fgOnBackground('var(--indigo)')).toBe('#0A0C10')
    expect(fgOnBackground('var(--coral)')).toBe('#0A0C10')
  })

  it('keeps dark ink on mid custom colors and flips only on dark fills', () => {
    expect(fgOnBackground('#eb7a5c')).toBe('#0A0C10')
    expect(fgOnBackground('#6b5bd4')).toBe('#0A0C10')
    expect(fgOnBackground('#1a1c22')).toBe('#F4F1EA')
    expect(fgOnBackground('#0A0C10')).toBe('#F4F1EA')
  })
})

describe('lucide catalog', () => {
  it('maps PascalCase exports to kebab-case names', () => {
    expect(lucideKebab('Rocket')).toBe('rocket')
    expect(lucideKebab('AArrowDown')).toBe('a-arrow-down')
    expect(lucideKebab('FolderGit')).toBe('folder-git')
    expect(lucideKebab('Axis3d')).toBe('axis-3d')
  })

  it('resolves current lucide names used by the picker', () => {
    expect(hasLucideIcon('box')).toBe(true)
    expect(hasLucideIcon('rocket')).toBe(true)
    expect(hasLucideIcon('folder-git')).toBe(true)
    expect(lucideNode('not-an-icon')).toBeUndefined()
    expect(lucideInnerHtml('box')).toContain('<path')
  })
})
