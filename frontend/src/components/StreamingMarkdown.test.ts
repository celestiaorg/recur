/** @vitest-environment happy-dom */
import { afterEach, expect, test, vi } from 'vitest'
import { flushSync, mount, unmount } from 'svelte'
import { fromStore, writable } from 'svelte/store'
import StreamingMarkdown from './StreamingMarkdown.svelte'
import { renderMarkdownAsync } from '../lib/markdown-host'

vi.mock('../lib/markdown-host', () => ({
  renderMarkdownAsync: vi.fn(async (src: string) => `<p>${src}</p>`),
}))

afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); vi.clearAllMocks() })

test('hidden streams do no presentation work, catch up on return, and wait for complete lines', async () => {
  const frames = new Map<number, FrameRequestCallback>()
  let id = 0
  vi.stubGlobal('requestAnimationFrame', (fn: FrameRequestCallback) => { frames.set(++id, fn); return id })
  vi.stubGlobal('cancelAnimationFrame', (id: number) => { frames.delete(id) })
  const state = fromStore(writable<{ text: string; active: boolean; complete?: boolean }>({ text: 'first reply', active: true, complete: false }))
  const target = document.createElement('div')
  document.body.append(target)
  const component = mount(StreamingMarkdown, {
    target,
    props: {
      get text() { return state.current.text },
      get active() { return state.current.active },
      get complete() { return !!state.current.complete },
    },
  })
  const settle = async () => { flushSync(); await Promise.resolve(); flushSync() }
  try {
    await settle()
    expect(frames.size).toBe(0)
    expect(target.textContent).toBe('first reply')

    vi.mocked(renderMarkdownAsync).mockClear()
    state.current = { text: 'hidden reply', active: false, complete: false }
    await settle()
    expect(renderMarkdownAsync).not.toHaveBeenCalled()
    state.current = { text: 'latest hidden reply', active: false }
    await settle()
    expect(renderMarkdownAsync).not.toHaveBeenCalled()

    state.current = { ...state.current, active: true }
    await settle()
    expect(target.textContent).toBe('latest hidden reply')
    expect(frames.size).toBe(0)

    state.current = { text: 'latest hidden reply\nplus a line\npartial', active: true }
    await settle()
    expect(target.textContent).toBe('latest hidden reply\nplus a line\n')
    expect(frames.size).toBe(0)

    state.current = { ...state.current, complete: true }
    await settle()
    expect(target.textContent).toBe(state.current.text)
    state.current = { ...state.current, complete: false }

    const hidden = vi.spyOn(document, 'hidden', 'get').mockReturnValue(true)
    document.dispatchEvent(new Event('visibilitychange'))
    const calls = vi.mocked(renderMarkdownAsync).mock.calls.length
    state.current = { text: 'reply while the app is hidden', active: true }
    await settle()
    expect(frames.size).toBe(0)
    expect(renderMarkdownAsync).toHaveBeenCalledTimes(calls)
    hidden.mockReturnValue(false)
    document.dispatchEvent(new Event('visibilitychange'))
    await settle()
    expect(target.textContent).toBe(state.current.text)
    expect(frames.size).toBe(0)

    const painted = target.textContent
    let finish!: (html: string) => void
    vi.mocked(renderMarkdownAsync).mockImplementationOnce(() => new Promise((resolve) => { finish = resolve }))
    state.current = { text: state.current.text + '\nmore\n', active: true }
    await settle()
    state.current = { ...state.current, active: false }
    await settle()
    finish('<p>late worker result</p>')
    await settle()
    expect(target.textContent).toBe(painted)
  } finally {
    await unmount(component)
    target.remove()
  }
})
