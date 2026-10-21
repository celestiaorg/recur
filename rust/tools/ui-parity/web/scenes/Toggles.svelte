<div class="wrap">
  <button class="tog" aria-label="off"></button>
  <button class="tog on" aria-label="on"></button>
  <div class="box"><div class="panel"></div></div>
  <div class="bar"><div class="prog"><div class="prog-fill" style="width: 40%"></div></div></div>
</div>

<style>
  .wrap { padding: 12px; display: flex; align-items: center; gap: 12px; }
  .box { width: 60px; height: 30px; display: flex; }
  .box > :global(.panel) { flex: 1; }
  .bar { width: 80px; }
</style>
