<script lang="ts">
  import WorkspaceDetail from "../src/views/WorkspaceDetail.svelte";
  import PRDetail from "../src/views/PRDetail.svelte";
  import Sidebar from "../src/components/Sidebar.svelte";
  import PRList from "../src/views/PRList.svelte";
  import Briefing from "../src/views/Briefing.svelte";
  import Toasts from "../src/components/Toasts.svelte";
  import Icon from "../src/components/Icon.svelte";
  import {
    view,
    prs,
    selectedPRId,
    workspaces,
    selectedWorkspaceId,
  } from "../src/lib/stores";
  const branch = $derived(
    $view === "prdetail"
      ? $prs.find((pr) => pr.id === $selectedPRId)?.branch
      : $workspaces.find((ws) => ws.id === $selectedWorkspaceId)?.branch,
  );
</script>

<!-- The product's workbench and PR views, without the desktop application shell. -->
<div class="win showcase">
  <header class="titlebar">
    <div class="crumb">
      <b>storefront</b><span> / </span><span
        >{$view === "prdetail"
          ? "Pull request #482"
          : $view === "briefing"
            ? "Inbox"
            : $view === "pr"
              ? "Pull requests"
              : $workspaces.find((ws) => ws.id === $selectedWorkspaceId)
                  ?.short}</span
      >
    </div>
    <span class="branch"><Icon name="git-line" />{branch}</span>
  </header>
  <div class="body">
    <Sidebar />
    <main class="main">
      <WorkspaceDetail />
      <PRDetail />
      <PRList />
      <Briefing />
    </main>
  </div>
</div>
<Toasts />

<style>
  .titlebar {
    padding: 0 18px;
    height: 40px;
    min-height: 40px;
    justify-content: space-between;
  }
  .branch {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--muted);
    font: 10px var(--mono);
  }
  .branch :global(svg) {
    width: 12px;
    height: 12px;
  }
  .main {
    min-height: 0;
  }

  /* simplify: this is a guided preview of one task. Keep the real panel DOM,
     typography and interactions; omit management and secondary dock controls.
     Add a corresponding demo interaction before exposing another control. */
  .showcase :global(.obs-tb-left > .obs-pane-toggle),
  .showcase :global(.rail-top .nav:last-child),
  .showcase :global(.rail .sa),
  .showcase :global(.rail .pacts),
  .showcase :global(.rail .tmain),
  .showcase :global(.rail .lact-rm),
  .showcase :global(.rail-foot-bar),
  .showcase :global(.briefing-ask-reopen),
  .showcase :global(.obs-tb-left [aria-label^="Plan"]),
  .showcase :global(.obs-tb-right),
  .showcase :global(.obs-term-toggle),
  .showcase :global(.wsv-chat-new),
  .showcase :global(.tab-chip-x),
  .showcase :global([aria-label="Split editor"]),
  .showcase :global([aria-label="Reload"]),
  .showcase :global(.obs-statusbar),
  .showcase :global(.prd-chat-toggle),
  .showcase :global(.prh .cmt-act),
  .showcase :global(.prh-side),
  .showcase :global(.cmt-more),
  .showcase :global(.finding-note-compose) {
    display: none;
  }

  .showcase :global(.prh-title) {
    white-space: normal;
  }
</style>
