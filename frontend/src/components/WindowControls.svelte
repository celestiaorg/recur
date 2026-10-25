<script lang="ts">
  import { onMount } from 'svelte'
  import { Window } from '@wailsio/runtime'
  import Icon from './Icon.svelte'

  let maximised = $state(false)

  const sync = () => void Window.IsMaximised().then((m) => (maximised = m)).catch(() => {})

  onMount(() => {
    sync()
    window.addEventListener('resize', sync)
    return () => window.removeEventListener('resize', sync)
  })
</script>

<div class="tb-winctl">
  <button type="button" class="tb-winbtn" aria-label="Minimize" title="Minimize" onclick={() => void Window.Minimise()}>
    <Icon name="minus" size={14} />
  </button>
  <button
    type="button"
    class="tb-winbtn"
    aria-label={maximised ? 'Restore' : 'Maximize'}
    title={maximised ? 'Restore' : 'Maximize'}
    onclick={() => void Window.ToggleMaximise()}
  >
    <Icon name={maximised ? 'window-restore' : 'square'} size={13} />
  </button>
  <button type="button" class="tb-winbtn tb-winclose" aria-label="Close" title="Close" onclick={() => void Window.Close()}>
    <Icon name="x" size={14} />
  </button>
</div>
