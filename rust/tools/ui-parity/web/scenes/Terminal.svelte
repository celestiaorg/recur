<script lang="ts">
  // The dock body as TerminalDock.svelte renders it (find bar markup copied verbatim, panes
  // around one TerminalPane), with the pane's session replaying the scene's input bytes
  // (capture.spec.ts answers AttachTerminal with them). Ready once the trailing OSC title has
  // been parsed and xterm has drawn.
  import { onMount } from 'svelte'
  import TerminalPane from '../../../../../frontend/src/components/TerminalPane.svelte'
  import Icon from '../../../../../frontend/src/components/Icon.svelte'
  import { terminalPrefs } from '../../../../../frontend/src/lib/stores'
  import { ensureTerminalThemes, terminalSurfaceBg } from '../../../../../frontend/src/lib/terminal'
  import { PREVIEW_FIND_CAP, previewFindCountLabel } from '../../../../../frontend/src/lib/preview-find'
  import scenes from '../../scenes.json'

  let { name, onready }: { name: string; onready: () => void } = $props()
  const spec = (scenes.terminal as { name: string; find?: boolean; terminalTheme?: string }[]).find(
    (s) => s.name === name,
  )
  const find = !!spec?.find

  let loaded = $state(false)
  let termBg = $state('')
  let pane: TerminalPane | undefined = $state()

  let findQuery = $state('')
  let findCase = $state(false)
  let findRegexp = $state(false)
  let findWord = $state(false)
  let findTotal = $state(0)
  let findIndex = $state(0)
  let findCapped = $state(false)
  let findValid = $state(true)
  const findCount = $derived(
    !findQuery || !findValid ? '' : previewFindCountLabel(findIndex, findTotal, findCapped),
  )

  onMount(async () => {
    await ensureTerminalThemes()
    terminalPrefs.update((p) => ({ ...p, theme: spec?.terminalTheme ?? '' }))
    termBg = terminalSurfaceBg(spec?.terminalTheme ?? '')
    loaded = true
  })

  function onTitle(title: string) {
    if (title !== 'ready') return
    let frames = 3
    const step = () => (--frames > 0 ? requestAnimationFrame(step) : onready())
    requestAnimationFrame(step)
  }

  function onFindResults(index: number, total: number, valid: boolean) {
    findValid = valid
    findIndex = index
    findTotal = total
    findCapped = total >= PREVIEW_FIND_CAP
  }

  function stepFind(dir: 1 | -1) {
    if (!pane || !findQuery) return
    const opts = { caseSensitive: findCase, regexp: findRegexp, wholeWord: findWord }
    if (dir > 0) pane.findNext(findQuery, opts)
    else pane.findPrevious(findQuery, opts)
  }

  $effect(() => {
    void findQuery
    void findCase
    void findRegexp
    void findWord
    if (!find || !pane) return
    if (!findQuery) {
      findTotal = 0
      findIndex = 0
      findCapped = false
      findValid = true
      pane.clearSearch()
      return
    }
    pane.findNext(findQuery, {
      caseSensitive: findCase, regexp: findRegexp, wholeWord: findWord, incremental: true,
    })
  })

  function onFindInputKey(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault()
      stepFind(e.shiftKey ? -1 : 1)
    }
  }
</script>

{#if loaded}
  <section class="term-dock" style:--term-bg={termBg}>
    <div class="term-dock-body">
      {#if find}
        <div class="term-dock-find cm-recur-search">
          <div class="cm-recur-search-row">
            <span class="cm-recur-search-glyph" aria-hidden="true"><Icon name="search" /></span>
            <input
              bind:value={findQuery}
              type="text"
              class="cm-recur-search-input"
              name="search"
              placeholder="Find"
              aria-label="Find"
              spellcheck="false"
              autocomplete="off"
              onkeydown={onFindInputKey}
            />
            <span
              class="cm-recur-search-count"
              class:none={!!findQuery && findValid && findTotal === 0}
              data-empty={!findQuery || !findValid ? '1' : '0'}
              aria-live="polite"
            >{findCount}</span>
            <div class="cm-recur-search-nav">
              <button type="button" class="obs-icon-btn" title="Previous match (⇧⌘G)" aria-label="Previous match" onclick={() => stepFind(-1)}>
                <Icon name="chevron-left" />
              </button>
              <button type="button" class="obs-icon-btn" title="Next match (⌘G)" aria-label="Next match" onclick={() => stepFind(1)}>
                <Icon name="chevron-right" />
              </button>
            </div>
            <div class="cm-recur-search-toggles">
              <button
                type="button"
                class="obs-icon-btn cm-recur-search-toggle"
                class:on={findCase}
                title="Match case (⌥⌘C)"
                aria-label="Match case"
                aria-pressed={findCase}
                onclick={() => { findCase = !findCase }}
              >
                <Icon name="case-sensitive" />
              </button>
              <button
                type="button"
                class="obs-icon-btn cm-recur-search-toggle"
                class:on={findRegexp}
                title="Regular expression (⌥⌘R)"
                aria-label="Regular expression"
                aria-pressed={findRegexp}
                onclick={() => { findRegexp = !findRegexp }}
              >
                <Icon name="regex" />
              </button>
              <button
                type="button"
                class="obs-icon-btn cm-recur-search-toggle"
                class:on={findWord}
                title="Whole word (⌥⌘W)"
                aria-label="Whole word"
                aria-pressed={findWord}
                onclick={() => { findWord = !findWord }}
              >
                <Icon name="whole-word" />
              </button>
            </div>
            <button type="button" class="obs-icon-btn cm-recur-search-close" title="Close" aria-label="Close find">
              <Icon name="x" />
            </button>
          </div>
        </div>
      {/if}
      <div class="term-dock-panes">
        <TerminalPane bind:this={pane} id={`parity-${name}`} workspaceId="parity" {onTitle} {onFindResults} />
      </div>
    </div>
  </section>
{/if}

<style>
  /* TerminalDock.svelte's own rules for these boxes (the dock's border and height belong to
     its chrome, which the scene leaves out). */
  .term-dock {
    position: relative;
    height: 100%;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--term-bg, var(--bg));
  }

  .term-dock-body {
    flex: 1;
    min-height: 0;
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }

  .term-dock-panes {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }
</style>
