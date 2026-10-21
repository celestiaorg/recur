<script lang="ts">
  import Ring from '../../../../../frontend/src/components/Ring.svelte'
</script>

<div class="wrap">
  <Ring size={30} prog={0.4} iter={2} />
  <Ring size={42} prog={0.8} iter={5} color="var(--emerald)" />
  <Ring size={60} prog={0} iter={0} />
  <Ring size={100} prog={0.6} iter={3} />
</div>

<style>
  .wrap { padding: 12px; display: flex; align-items: center; gap: 14px; }
</style>
