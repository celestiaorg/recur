<script lang="ts">
  import manifest from "../../../public/screenshots/manifest.json";

  let {
    screen,
    theme,
    eager = false,
  }: {
    screen:
      "workspace" | "changes" | "loop" | "pull-request" | "inbox" | "schedules";
    theme: "paper" | "charcoal";
    eager?: boolean;
  } = $props();

  const capture = $derived(
    manifest.captures.find((item) => item.id === `${screen}-${theme}`)!,
  );
  let dialog: HTMLDialogElement;
  const displayWidth = $derived(
    capture.dimensions.width / capture.settings.deviceScaleFactor,
  );
  const displayHeight = $derived(
    capture.dimensions.height / capture.settings.deviceScaleFactor,
  );
  const src = $derived(`/screenshots/${capture.file}`);
</script>

<button
  class="capture-trigger"
  type="button"
  onclick={() => dialog.showModal()}
  aria-label="Enlarge {capture.title} in {theme}"
>
  <img
    {src}
    alt="{capture.title} in Recur’s {theme === 'paper'
      ? 'Paper'
      : 'Charcoal'} theme"
    width={displayWidth}
    height={displayHeight}
    loading={eager ? "eager" : "lazy"}
    decoding="async"
  />
</button>

<dialog
  bind:this={dialog}
  aria-label={capture.title}
  style:--image-width="{displayWidth}px"
>
  <header>
    <span>{capture.title}</span><button
      type="button"
      onclick={() => dialog.close()}
      aria-label="Close screenshot">Close ×</button
    >
  </header>
  <img
    {src}
    alt="{capture.title}, enlarged"
    width={displayWidth}
    height={displayHeight}
    loading="lazy"
  />
</dialog>

<style>
  .capture-trigger {
    width: 100%;
    padding: 0;
    border: 0;
    background: transparent;
    display: block;
    cursor: zoom-in;
  }
  img {
    display: block;
    width: 100%;
    height: auto;
  }
  dialog {
    box-sizing: border-box;
    width: calc(var(--image-width) + 24px);
    max-width: calc(100vw - 32px);
    max-height: calc(100dvh - 32px);
    margin: auto;
    padding: 12px;
    overflow: auto;
    border: 1px solid var(--line-bright);
    border-radius: 10px;
    background: var(--pane);
    color: var(--text);
  }
  dialog::backdrop {
    background: #000b;
  }
  header {
    position: sticky;
    top: -12px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: 12px 0;
    background: var(--pane);
    font-size: 13px;
  }
  header button {
    flex: none;
    padding: 8px 12px;
    border-radius: 5px;
    background: var(--pane-2);
    color: var(--text);
  }
</style>
