<script lang="ts">
  import Toasts from '../../../../../frontend/src/components/Toasts.svelte'
  import { pushAskToast, pushToast, toasts } from '../../../../../frontend/src/lib/stores'

  toasts.set([])
  pushToast('success', 'Saved workspace settings', { ttlMs: 0 })
  pushToast('error', 'Push failed: the remote rejected the update', { ttlMs: 0 })
  pushToast('info', 'Recur 0.9 is ready', { action: { label: 'Restart', fn: () => {} }, ttlMs: 0 })
  pushAskToast(
    { project: 'recur', workspace: 'honeyed-cannoli', question: 'Should the parity task also run on Linux?' },
    { label: 'Answer', fn: () => {} },
  )
</script>

<Toasts />
