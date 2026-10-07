<script lang="ts">
  import type { AppTheme, EditorTheme } from "../themes";
  import { defaultView, type PreviewView } from "../preview";

  let {
    app,
    editor,
    view = $bindable({ ...defaultView }),
  }: {
    app: AppTheme;
    editor: EditorTheme;
    view?: PreviewView;
  } = $props();
  let frame = $state<HTMLIFrameElement>();
  let ready = $state(false);

  let lastSentView: PreviewView | undefined;

  function sync() {
    if (ready) {
      frame?.contentWindow?.postMessage(
        {
          type: "recur-preview",
          navigate: lastSentView !== view,
          app: app.key,
          editor: editor.key,
          ...view,
        },
        location.origin,
      );
      lastSentView = view;
    }
  }
  $effect(sync);

  function receive(event: MessageEvent) {
    if (
      event.source !== frame?.contentWindow ||
      event.origin !== location.origin
    )
      return;
    if (event.data?.type === "recur-preview-ready") ready = true;
  }
</script>

<svelte:window onmessage={receive} />
<iframe
  bind:this={frame}
  src="/demo/showcase/index.html"
  title="Interactive Recur: {app.name} app, {editor.name} editor"
  loading="lazy"
  sandbox="allow-scripts allow-same-origin"
  onload={() => sync()}
></iframe>

<style>
  iframe {
    display: block;
    width: 100%;
    height: 100%;
    border: 0;
    background: var(--pane);
  }
</style>
