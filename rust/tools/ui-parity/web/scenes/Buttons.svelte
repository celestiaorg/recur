<script lang="ts">
  import Icon from '../../../../../frontend/src/components/Icon.svelte'
</script>

<div class="wrap">
  <div class="row">
    <button class="tbtn">Default</button>
    <button class="tbtn amber">Amber</button>
    <button class="tbtn solid">Solid</button>
    <button class="tbtn on">On</button>
    <button class="tbtn danger">Remove</button>
    <button class="tbtn danger armed">Confirm</button>
    <button class="tbtn"><Icon name="plus" />Create workspace</button>
  </div>
  <div class="row">
    <div class="tb-right">
      <button class="tbtn">Titlebar</button>
      <button class="tbtn danger">Remove</button>
      <button class="tbtn danger" disabled>Disabled</button>
    </div>
  </div>
  <div class="row">
    <button class="rbtn">Review</button>
    <button class="rbtn merge">Merge</button>
    <button class="rbtn primary">Primary</button>
    <button class="rbtn armed">Armed</button>
    <button class="rbtn primary" disabled>Disabled</button>
  </div>
  <div class="row">
    <button class="nt-go">Create</button>
    <button class="nt-go" disabled>Create</button>
    <button class="nt-link"><Icon name="plus" />Attach</button>
    <button class="nt-link on"><Icon name="plus" />Linked</button>
    <span class="lnk">open log</span>
  </div>
  <div class="row">
    <button class="tb-icon-btn"><Icon name="plus" /></button>
    <button class="tb-icon-btn on"><Icon name="plus" /></button>
    <button class="tb-rail"><Icon name="box" /></button>
    <button class="tb-rail on"><Icon name="box" /></button>
  </div>
</div>

<style>
  .wrap { padding: 12px; display: flex; flex-direction: column; gap: 10px; }
  .row { display: flex; align-items: center; gap: 8px; }
</style>
