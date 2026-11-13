<script lang="ts">
  import type { AppTheme, EditorTheme } from "../themes";
  import shot from "../../../public/screenshots/themes/manifest.json";

  let { app, editor }: { app: AppTheme; editor: EditorTheme } = $props();

  // The editor's code area, laid over the app shot where `mise run capture:splash` cropped it.
  const pct = (n: number, of: number) => `${(n / of) * 100}%`;
  const e = shot.editor;
</script>

<div
  class="theme-shot"
  style:aspect-ratio="{shot.width} / {shot.height}"
  role="img"
  aria-label="Recur with the {app.name} app theme and the {editor.name} editor theme"
>
  <img src="/screenshots/themes/app-{app.key}.png" alt="" decoding="async" />
  <img
    class="editor"
    src="/screenshots/themes/editor-{editor.key}.png"
    alt=""
    decoding="async"
    style:left={pct(e.x, shot.width)}
    style:top={pct(e.y, shot.height)}
    style:width={pct(e.width, shot.width)}
  />
</div>

<style>
  .theme-shot {
    position: relative;
    width: 100%;
    background: var(--pane);
  }
  img {
    display: block;
    width: 100%;
    height: auto;
  }
  .editor {
    position: absolute;
  }
</style>
