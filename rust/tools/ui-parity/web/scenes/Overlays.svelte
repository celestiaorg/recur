<script lang="ts">
  import Icon from '../../../../../frontend/src/components/Icon.svelte'
  import Kbd from '../../../../../frontend/src/components/Kbd.svelte'
  import ProjectPic from '../../../../../frontend/src/components/ProjectPic.svelte'

  let { variant }: { variant: string } = $props()
</script>

{#if variant === 'palette'}
  <div class="palette-bg open">
    <div class="palette">
      <div class="pal-in">
        <Icon name="search" />
        <input placeholder="Search workspaces, PRs, or projects…" />
        <span class="esc">esc</span>
      </div>
      <div class="pal-list">
        <div class="pal-g">Workspaces</div>
        <div class="pal-row sel">
          <span class="pic"><Icon name="git-branch" /></span>
          <span class="pl">honeyed-cannoli</span>
          <span class="psub">recur</span>
          <Kbd class="pk" text="⌘1" />
        </div>
        <div class="pal-row">
          <span class="pic"><span class="dot" style="background: var(--emerald)"></span></span>
          <span class="pl">main</span>
          <span class="psub">recur</span>
        </div>
        <div class="pal-g">Projects</div>
        <div class="pal-row">
          <span class="pic"><ProjectPic pic="" color="var(--cyan)" name="recur" /></span>
          <span class="pl">recur</span>
          <Kbd class="pk" text="⌘⇧R" />
        </div>
      </div>
      <div class="pal-foot">
        <span><Kbd class="k" text="↑↓" />navigate</span><span><Kbd class="k" text="↵" />open</span><span><Kbd class="k" text="esc" />close</span>
      </div>
    </div>
  </div>
{:else if variant === 'modal'}
  <div class="nt-bg">
    <div class="nt-modal">
      <div class="body">New workspace</div>
    </div>
  </div>
{:else}
  <div class="wrap">
    <div class="col">
      <input class="inp" placeholder="~/Development/recur" />
      <input class="inp" value="" placeholder="read only" readonly />
      <div class="skelrow"><div class="skel a"></div><div class="skel b"></div></div>
      <div class="skelrow"><div class="skel a"></div><div class="skel c"></div></div>
      <div class="prog"><div class="prog-fill" style="width: 40%"></div></div>
      <div class="hint">Nothing here yet</div>
    </div>
    <div class="pop"><div class="nt-tip-pop">Workspaces get their own branch and worktree, so agents never collide.</div></div>
  </div>
{/if}

<style>
  .body { padding: 20px; font-size: 14px; }
  .wrap { padding: 16px; display: flex; gap: 16px; align-items: flex-start; }
  .col { width: 260px; display: flex; flex-direction: column; gap: 10px; }
  .a { width: 24px; height: 24px; }
  .b { width: 140px; }
  .c { width: 90px; }
  .pop { position: relative; margin-top: 140px; }
</style>
