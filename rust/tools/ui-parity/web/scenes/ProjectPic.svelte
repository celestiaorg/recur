<script lang="ts">
  import ProjectPic from '../../../../../frontend/src/components/ProjectPic.svelte'
  const pics = [
    { pic: '', color: 'var(--cyan)', name: 'recur' },
    { pic: 'Ab', color: 'var(--amber)', name: '' },
    { pic: 'lucide:rocket', color: 'var(--indigo)', name: '' },
    { pic: 'lucide:box', color: '#1e2a3a', name: '' },
    { pic: '', color: '#f2c94c', name: 'zeta' },
    { pic: '', color: '', name: 'émile' },
  ]
</script>

<div class="wrap">
  {#each pics as p}<ProjectPic class="nt-pic" pic={p.pic} color={p.color} name={p.name} />{/each}
</div>

<style>
  .wrap { padding: 12px; display: flex; align-items: center; gap: 10px; }
</style>
