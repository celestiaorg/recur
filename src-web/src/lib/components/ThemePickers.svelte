<script lang="ts">
  import { appThemes, editorThemes } from "../themes";

  let {
    appKey = $bindable("default"),
    editorKey = $bindable("recur"),
  }: { appKey?: string; editorKey?: string } = $props();

  let appScrollEl = $state<HTMLDivElement | undefined>();
  let editorScrollEl = $state<HTMLDivElement | undefined>();
  let appFade = $state({ left: false, right: false });
  let editorFade = $state({ left: false, right: false });

  function scrollFade(el: HTMLElement) {
    return {
      left: el.scrollLeft > 1,
      right: el.scrollLeft + el.clientWidth < el.scrollWidth - 1,
    };
  }

  function watchScroll(
    el: HTMLDivElement | undefined,
    set: (v: { left: boolean; right: boolean }) => void,
  ) {
    if (!el) return;
    const sync = () => set(scrollFade(el));
    sync();
    el.addEventListener("scroll", sync, { passive: true });
    const ro = new ResizeObserver(sync);
    ro.observe(el);
    return () => {
      el.removeEventListener("scroll", sync);
      ro.disconnect();
    };
  }

  $effect(() => watchScroll(appScrollEl, (v) => (appFade = v)));
  $effect(() => watchScroll(editorScrollEl, (v) => (editorFade = v)));
</script>

<div class="theme-pickers">
  <div class="theme-picker">
    <div class="cust-label theme-picker-label">App</div>
    <div
      class="theme-scroll"
      class:fade-l={appFade.left}
      class:fade-r={appFade.right}
    >
      <div
        class="theme-row"
        role="group"
        aria-label="App theme"
        bind:this={appScrollEl}
      >
        {#each appThemes as t}
          <button
            type="button"
            class="theme-swatch"
            class:on={t.key === appKey}
            aria-pressed={t.key === appKey}
            title={t.name}
            onclick={() => (appKey = t.key)}
          >
            <span
              class="theme-chip"
              style:background="linear-gradient(145deg, {t.bg} 55%, {t.pane} 55%)"
              aria-hidden="true"
            >
              <i style:background={t.accent}></i>
            </span>
            <span class="theme-swatch-name">{t.name}</span>
          </button>
        {/each}
      </div>
    </div>
  </div>

  <div class="theme-picker">
    <div class="cust-label theme-picker-label">Editor</div>
    <div
      class="theme-scroll"
      class:fade-l={editorFade.left}
      class:fade-r={editorFade.right}
    >
      <div
        class="theme-row"
        role="group"
        aria-label="Editor theme"
        bind:this={editorScrollEl}
      >
        {#each editorThemes as t}
          <button
            type="button"
            class="theme-swatch"
            class:on={t.key === editorKey}
            aria-pressed={t.key === editorKey}
            title={t.name}
            onclick={() => (editorKey = t.key)}
          >
            <span
              class="theme-chip editor"
              style:background={t.bg}
              aria-hidden="true"
            >
              <i style:background={t.a}></i>
              <i style:background={t.b}></i>
              <i style:background={t.c}></i>
            </span>
            <span class="theme-swatch-name">{t.name}</span>
          </button>
        {/each}
      </div>
    </div>
  </div>
</div>
