/** @vitest-environment happy-dom */
import { describe, expect, test } from 'vitest'
import {
  deleteAdjacentComposerChip,
  ensureChipBoundariesInDom,
  ensureComposerCaret,
  hydrateComposer,
  makeArtifactChip,
  makeFileChip,
  makeTerminalChip,
  normalizeComposerDom,
  serializeComposer,
  composerCaretClientRect,
} from './composerDom'

describe('makeFileChip', () => {
  test('stores the full chip path and shows a line-span label', () => {
    const chip = makeFileChip('src/a.ts:12-40')
    expect(chip.dataset.filePath).toBe('src/a.ts:12-40')
    expect(chip.title).toBe('Open src/a.ts:12-40')
    expect(chip.querySelector('.chat-composer-chip-name')?.textContent).toBe('a.ts (12-40)')
    expect(chip.getAttribute('aria-label')).toBe('Open src/a.ts:12-40')
  })

  test('directory chips use a folder glyph', () => {
    const chip = makeFileChip('src/lib/')
    expect(chip.dataset.filePath).toBe('src/lib/')
    expect(chip.dataset.dir).toBe('1')
    expect(chip.title).toBe('src/lib')
    expect(chip.getAttribute('aria-label')).toBe('src/lib')
    expect(chip.getAttribute('role')).toBeNull()
    expect(chip.querySelector('.folder-ico')).toBeTruthy()
    expect(chip.querySelector('.fico')).toBeNull()
  })

  test('plain path uses basename label', () => {
    const chip = makeFileChip('frontend/src/lib/agentchat.ts')
    expect(chip.dataset.filePath).toBe('frontend/src/lib/agentchat.ts')
    expect(chip.querySelector('.chat-composer-chip-name')?.textContent).toBe('agentchat.ts')
  })

  test('column-span chips show line:col-line:col in the label', () => {
    const same = makeFileChip('src/a.ts:12:5-12:18')
    expect(same.dataset.filePath).toBe('src/a.ts:12:5-12:18')
    expect(same.querySelector('.chat-composer-chip-name')?.textContent).toBe('a.ts (12:5-12:18)')
    const cross = makeFileChip('src/a.ts:12:5-13:20')
    expect(cross.querySelector('.chat-composer-chip-name')?.textContent).toBe('a.ts (12:5-13:20)')
  })
})

describe('hydrateComposer / serializeComposer', () => {
  test('round-trips line-span @file chips', () => {
    const root = document.createElement('div')
    const wire = 'see @src/a.ts:12-40 please'
    hydrateComposer(root, wire, [], { chipFiles: true })
    expect(serializeComposer(root)).toBe(wire)
    const chip = root.querySelector<HTMLElement>('[data-file-path]')
    expect(chip?.dataset.filePath).toBe('src/a.ts:12-40')
    expect(chip?.querySelector('.chat-composer-chip-name')?.textContent).toBe('a.ts (12-40)')
  })

  test('round-trips column-span @file chips', () => {
    const root = document.createElement('div')
    const wire = 'see @src/a.ts:12:5-12:18 please'
    hydrateComposer(root, wire, [], { chipFiles: true })
    expect(serializeComposer(root)).toBe(wire)
    const chip = root.querySelector<HTMLElement>('[data-file-path]')
    expect(chip?.dataset.filePath).toBe('src/a.ts:12:5-12:18')
    expect(chip?.querySelector('.chat-composer-chip-name')?.textContent).toBe('a.ts (12:5-12:18)')
  })

  test('round-trips plain @file chips at start and end', () => {
    const root = document.createElement('div')
    hydrateComposer(root, '@PROGRESS.md please review', [], { chipFiles: true })
    expect(serializeComposer(root)).toBe('@PROGRESS.md please review')

    hydrateComposer(root, 'please review @PROGRESS.md', [], { chipFiles: true })
    expect(serializeComposer(root)).toBe('please review @PROGRESS.md')
  })

  test('round-trips directory chips including bare root names', () => {
    const root = document.createElement('div')
    const wire = 'look at @src/ and @src/lib/'
    hydrateComposer(root, wire, [], { chipFiles: true })
    expect(serializeComposer(root)).toBe(wire)
    const chips = [...root.querySelectorAll<HTMLElement>('[data-file-path]')]
    expect(chips.map((c) => c.dataset.filePath)).toEqual(['src/', 'src/lib/'])
    expect(chips.every((c) => c.dataset.dir === '1')).toBe(true)
    expect(chips.every((c) => c.querySelector('.folder-ico'))).toBe(true)
  })

  test('round-trips dotted root folder chips', () => {
    const root = document.createElement('div')
    const p = document.createElement('p')
    p.appendChild(makeFileChip('.github/'))
    root.appendChild(p)
    const wire = serializeComposer(root)
    expect(wire).toBe('@.github/')
    hydrateComposer(root, '@.github/ and @foo.bar/', [], { chipFiles: true })
    expect(serializeComposer(root)).toBe('@.github/ and @foo.bar/')
    const chips = [...root.querySelectorAll<HTMLElement>('[data-file-path]')]
    expect(chips.map((c) => c.dataset.filePath)).toEqual(['.github/', 'foo.bar/'])
    expect(chips.every((c) => c.dataset.dir === '1')).toBe(true)
    expect(chips.every((c) => c.getAttribute('role') === null)).toBe(true)
    expect(root.querySelector('.fico')).toBeNull()
    expect([...root.querySelectorAll('p')].flatMap((el) => [...el.childNodes])
      .filter((n) => n.nodeType === Node.TEXT_NODE)
      .map((n) => n.textContent)).toEqual([' and '])
  })

  test('reserved directory names stay file chips, not @terminal', () => {
    const root = document.createElement('div')
    hydrateComposer(root, 'see @terminal/', [], { chipFiles: true })
    expect(serializeComposer(root)).toBe('see @terminal/')
    expect(root.querySelector('[data-terminal-chip]')).toBeNull()
    const chip = root.querySelector<HTMLElement>('[data-file-path]')
    expect(chip?.dataset.filePath).toBe('terminal/')
    expect(chip?.dataset.dir).toBe('1')
  })

  test('chipFiles hydrate repairs missing boundaries so chips are restored', () => {
    const root = document.createElement('div')
    hydrateComposer(root, 'what does it do?@frontend/src/a.ts:12-40', [], { chipFiles: true })
    expect(serializeComposer(root)).toBe('what does it do? @frontend/src/a.ts:12-40')
    expect(root.querySelector<HTMLElement>('[data-file-path]')?.dataset.filePath).toBe(
      'frontend/src/a.ts:12-40',
    )
  })
})

describe('ensureComposerCaret', () => {
  test('parks caret inside the empty <p> after clear-hydrate (not on the root)', () => {
    const root = document.createElement('div')
    root.contentEditable = 'true'
    document.body.appendChild(root)
    hydrateComposer(root, '/fix-findings', [], { chipSkills: true })
    hydrateComposer(root, '', [], { chipFiles: true, chipSkills: true })

    // Simulate WebKit after replaceChildren: selection collapses onto the root.
    const sel = window.getSelection()!
    const broken = document.createRange()
    broken.setStart(root, 0)
    broken.collapse(true)
    sel.removeAllRanges()
    sel.addRange(broken)
    expect(sel.focusNode).toBe(root)

    ensureComposerCaret(root, 0)

    const p = root.querySelector('p')
    expect(p).toBeTruthy()
    expect(p!.contains(sel.focusNode)).toBe(true)
    expect(sel.focusNode).not.toBe(root)
    root.remove()
  })
})

describe('ensureChipBoundariesInDom', () => {
  test('reinserts a leading space when text sits flush against a chip', () => {
    const root = document.createElement('div')
    const p = document.createElement('p')
    p.appendChild(document.createTextNode('hello'))
    p.appendChild(makeFileChip('src/a.ts:1-2'))
    root.appendChild(p)
    expect(serializeComposer(root)).toBe('hello@src/a.ts:1-2')
    ensureChipBoundariesInDom(root)
    expect(serializeComposer(root)).toBe('hello @src/a.ts:1-2')
  })

  test('separates flush chips with a space', () => {
    const root = document.createElement('div')
    const p = document.createElement('p')
    p.appendChild(makeFileChip('src/a.ts'))
    p.appendChild(makeFileChip('src/b.ts'))
    root.appendChild(p)
    ensureChipBoundariesInDom(root)
    expect(serializeComposer(root)).toBe('@src/a.ts @src/b.ts')
  })
})

describe('artifact chips', () => {
  test('serialize round-trips @artifact:id', () => {
    const chip = makeArtifactChip('abc-def', 'Notes')
    expect(chip.dataset.artifactId).toBe('abc-def')
    expect(chip.querySelector('.chat-composer-chip-name')?.textContent).toBe('Notes')
    const root = document.createElement('div')
    hydrateComposer(root, 'see @artifact:abc-def', [], {
      chipFiles: true,
      artifactTitle: (id) => (id === 'abc-def' ? 'Notes' : ''),
    })
    expect(serializeComposer(root)).toBe('see @artifact:abc-def')
    expect(root.querySelector('[data-artifact-chip]')?.querySelector('.chat-composer-chip-name')?.textContent).toBe('Notes')
  })

  test('bare @artifact hydrates', () => {
    const root = document.createElement('div')
    hydrateComposer(root, 'see @artifact', [], { chipFiles: true })
    expect(serializeComposer(root)).toBe('see @artifact')
    expect(root.querySelector('[data-artifact-chip]')).toBeTruthy()
  })

  test('hydrates @Title when the panel name is unique', () => {
    const root = document.createElement('div')
    hydrateComposer(root, 'see @Notes please', [], {
      chipFiles: true,
      artifactIdForTitle: (t) => (t.toLowerCase() === 'notes' ? 'abc-def' : ''),
      artifactTitle: (id) => (id === 'abc-def' ? 'Notes' : ''),
    })
    expect(serializeComposer(root)).toBe('see @artifact:abc-def please')
    expect(root.querySelector('[data-artifact-chip] .chat-composer-chip-name')?.textContent).toBe('Notes')
  })

  test('leaves unknown @Name as prose', () => {
    const root = document.createElement('div')
    hydrateComposer(root, 'ping @nobody', [], {
      chipFiles: true,
      artifactIdForTitle: () => '',
    })
    expect(serializeComposer(root)).toBe('ping @nobody')
    expect(root.querySelector('[data-artifact-chip]')).toBeNull()
  })

  test('inserts a trailing space after an artifact chip flush against text', () => {
    const root = document.createElement('div')
    const p = document.createElement('p')
    p.appendChild(makeArtifactChip('abc-def', 'Notes'))
    p.appendChild(document.createTextNode('why'))
    root.appendChild(p)
    expect(serializeComposer(root)).toBe('@artifact:abc-defwhy')
    ensureChipBoundariesInDom(root)
    expect(serializeComposer(root)).toBe('@artifact:abc-def why')
  })
})

describe('terminal chips', () => {
  test('serialize round-trips @terminal:id', () => {
    const chip = makeTerminalChip('abc-def', 'dev')
    expect(chip.dataset.terminalId).toBe('abc-def')
    expect(chip.querySelector('.chat-composer-chip-name')?.textContent).toBe('dev')
    const root = document.createElement('div')
    hydrateComposer(root, 'fix @terminal:abc-def', [], {
      chipFiles: true,
      terminalTitle: (id) => (id === 'abc-def' ? 'dev' : ''),
    })
    expect(serializeComposer(root)).toBe('fix @terminal:abc-def')
    expect(root.querySelector('[data-terminal-chip]')?.querySelector('.chat-composer-chip-name')?.textContent).toBe('dev')
  })

  test('bare @terminal hydrates', () => {
    const root = document.createElement('div')
    hydrateComposer(root, 'see @terminal', [], { chipFiles: true })
    expect(serializeComposer(root)).toBe('see @terminal')
    expect(root.querySelector('[data-terminal-chip]')).toBeTruthy()
  })

  test('inserts a trailing space after a terminal chip flush against text', () => {
    const root = document.createElement('div')
    const p = document.createElement('p')
    p.appendChild(makeTerminalChip('abc-def', 'dev'))
    p.appendChild(document.createTextNode('why'))
    root.appendChild(p)
    expect(serializeComposer(root)).toBe('@terminal:abc-defwhy')
    ensureChipBoundariesInDom(root)
    expect(serializeComposer(root)).toBe('@terminal:abc-def why')
  })
})

describe('prompt chips', () => {
  const spec = { slash: 'linear', args: [{ name: 'issue', placeholder: 'TEAM-123' }] }

  test('hydrate does not skill-chip a takesArgs prompt', () => {
    const root = document.createElement('div')
    hydrateComposer(root, '/linear ENG-1', [], { chipSkills: true, promptChips: [spec] })
    expect(root.querySelector('[data-skill-name]')).toBeNull()
    const chip = root.querySelector<HTMLElement>('[data-prompt-slash]')
    expect(chip?.dataset.promptSlash).toBe('linear')
    expect(serializeComposer(root)).toBe('/linear ENG-1')
    const input = chip?.querySelector<HTMLInputElement>('.chat-composer-prompt-arg-input')
    expect(input?.value).toBe('ENG-1')
    expect(chip?.querySelector('.chat-composer-prompt-arg-label')?.textContent).toBe('issue')
  })

  test('empty args serialize with a trailing space', () => {
    const root = document.createElement('div')
    hydrateComposer(root, '/linear ', [], { chipSkills: true, promptChips: [spec] })
    expect(serializeComposer(root)).toBe('/linear ')
  })

  test('normalizeComposerDom does not steal focus from a prompt arg input', () => {
    const root = document.createElement('div')
    document.body.appendChild(root)
    hydrateComposer(root, '/linear ', [], { chipSkills: true, promptChips: [spec] })
    const input = root.querySelector<HTMLInputElement>('.chat-composer-prompt-arg-input')
    expect(input).toBeTruthy()
    input!.focus()
    input!.value = 'E'
    input!.dispatchEvent(new Event('input', { bubbles: true }))
    normalizeComposerDom(root)
    expect(document.activeElement).toBe(input)
    expect(serializeComposer(root)).toBe('/linear E')
    root.remove()
  })

  test('multi-arg hydrate keeps labels out of the wire form', () => {
    const multi = {
      slash: 'poll',
      args: [
        { name: 'question', placeholder: 'q' },
        { name: 'choice_1', placeholder: 'a' },
        { name: 'choice_2', placeholder: 'b' },
      ],
    }
    const root = document.createElement('div')
    hydrateComposer(root, '/poll Which Red Blue', [], { chipSkills: true, promptChips: [multi] })
    expect(root.querySelector('[data-skill-name]')).toBeNull()
    expect(serializeComposer(root)).toBe('/poll Which Red Blue')
    const labels = [...root.querySelectorAll('.chat-composer-prompt-arg-label')].map((el) => el.textContent)
    expect(labels).toEqual(['question', 'choice_1', 'choice_2'])
    const values = [...root.querySelectorAll<HTMLInputElement>('.chat-composer-prompt-arg-input')].map((el) => el.value)
    expect(values).toEqual(['Which', 'Red', 'Blue'])
  })

  test('quoted multi-arg values round-trip spaces', () => {
    const multi = {
      slash: 'poll',
      args: [
        { name: 'question', placeholder: 'q' },
        { name: 'choice_1', placeholder: 'a' },
        { name: 'choice_2', placeholder: 'b' },
      ],
    }
    const root = document.createElement('div')
    hydrateComposer(root, '/poll', [], { chipSkills: true, promptChips: [multi] })
    const inputs = [...root.querySelectorAll<HTMLInputElement>('.chat-composer-prompt-arg-input')]
    inputs[0]!.value = 'Which color?'
    inputs[1]!.value = 'Red'
    inputs[2]!.value = 'Blue'
    const wire = serializeComposer(root)
    expect(wire).toBe('/poll "Which color?" Red Blue')
    hydrateComposer(root, wire, [], { chipSkills: true, promptChips: [multi] })
    const again = [...root.querySelectorAll<HTMLInputElement>('.chat-composer-prompt-arg-input')].map((el) => el.value)
    expect(again).toEqual(['Which color?', 'Red', 'Blue'])
  })

  test('artifact chip counts in caret wire length', () => {
    const root = document.createElement('div')
    document.body.appendChild(root)
    hydrateComposer(root, 'see @artifact:abc-def x', [], { chipFiles: true })
    ensureComposerCaret(root, 'see @artifact:abc-def x'.length)
    const sel = window.getSelection()
    expect(sel?.focusNode?.textContent).toContain('x')
    root.remove()
  })

  test('backspace in a prompt arg input does not delete the chip', () => {
    const root = document.createElement('div')
    root.contentEditable = 'true'
    document.body.appendChild(root)
    hydrateComposer(root, '/linear ENG', [], { chipSkills: true, promptChips: [spec] })
    const input = root.querySelector<HTMLInputElement>('.chat-composer-prompt-arg-input')
    expect(input).toBeTruthy()
    input!.focus()
    input!.value = 'ENG'
    input!.setSelectionRange(3, 3)
    expect(deleteAdjacentComposerChip(root, 'Backspace')).toBeNull()
    expect(deleteAdjacentComposerChip(root, 'Delete')).toBeNull()
    expect(root.querySelector('[data-prompt-slash]')).toBeTruthy()
    expect(serializeComposer(root)).toBe('/linear ENG')
    root.remove()
  })
})

describe('composerCaretClientRect', () => {
  test('returns null when the composer has no selection', () => {
    const root = document.createElement('div')
    root.contentEditable = 'true'
    root.textContent = '/com'
    document.body.appendChild(root)
    expect(composerCaretClientRect(root)).toBeNull()
    root.remove()
  })
})
