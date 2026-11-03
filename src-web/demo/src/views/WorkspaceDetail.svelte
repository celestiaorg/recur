<script lang="ts">
  import { view, selectedWorkspaceId } from '../lib/stores'
  import WorkspaceView from './WorkspaceView.svelte'
</script>

<section
  class="view"
  class:on={$view === 'detail'}
  id="v-detail"
  inert={$view !== 'detail' || undefined}
  aria-hidden={$view !== 'detail' || undefined}
>
  {#key $selectedWorkspaceId}
    {#if $selectedWorkspaceId}
      <div class="wsv-live"><WorkspaceView workspaceId={$selectedWorkspaceId} /></div>
    {/if}
  {/key}
</section>
