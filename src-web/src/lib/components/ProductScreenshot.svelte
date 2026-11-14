<script lang="ts">
  import manifest from "../../../public/screenshots/manifest.json";

  let {
    screen,
    eager = false,
    caption,
    position,
    onprev,
    onnext,
  }: {
    screen:
      | "workspace"
      | "changes"
      | "loop"
      | "pull-request"
      | "inbox"
      | "issues"
      | "schedules";
    eager?: boolean;
    /** Shown in the enlarged view; defaults to the capture title. */
    caption?: string;
    /** Slideshow counter such as "2 / 7"; with onprev/onnext, adds paging. */
    position?: string;
    onprev?: () => void;
    onnext?: () => void;
  } = $props();
  const slideshow = $derived(Boolean(onprev && onnext));

  function onkeydown(event: KeyboardEvent) {
    if (!slideshow) return;
    if (event.key === "ArrowLeft") {
      event.preventDefault();
      onprev!();
    } else if (event.key === "ArrowRight") {
      event.preventDefault();
      onnext!();
    }
  }

  const capture = $derived(
    manifest.captures.find((item) => item.id === screen)!,
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
  aria-label="Enlarge {capture.title}"
>
  <img
    {src}
    alt={capture.title}
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
  {onkeydown}
>
  <header>
    <p aria-live="polite">{caption ?? capture.title}</p>
    <div class="controls">
      {#if slideshow}
        {#if position}<span class="position">{position}</span>{/if}
        <button type="button" onclick={onprev} aria-label="Previous screenshot"
          ><svg
            aria-hidden="true"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"><path d="m15 18-6-6 6-6" /></svg
          ></button
        ><button type="button" onclick={onnext} aria-label="Next screenshot"
          ><svg
            aria-hidden="true"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"><path d="m9 18 6-6-6-6" /></svg
          ></button
        >
      {/if}
      <button
        type="button"
        onclick={() => dialog.close()}
        aria-label="Close screenshot">Close ×</button
      >
    </div>
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
  header p {
    line-height: 1.5;
  }
  .controls {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: none;
  }
  .position {
    margin-right: 6px;
    font: 12px var(--mono);
    color: var(--muted);
  }
  header button {
    display: inline-flex;
    align-items: center;
    padding: 8px 12px;
    border-radius: 5px;
    background: var(--pane-2);
    color: var(--text);
  }
  header button:hover {
    background: var(--line);
  }
  header button svg {
    width: 16px;
    height: 16px;
  }
</style>
