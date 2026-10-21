<script lang="ts">
  import ArmedButton from '../../../../../frontend/src/components/ArmedButton.svelte'
  import DecodeText from '../../../../../frontend/src/components/DecodeText.svelte'
  import Icon from '../../../../../frontend/src/components/Icon.svelte'
</script>

<div class="wrap">
  <div class="row">
    <ArmedButton label="Remove" confirmLabel="Confirm remove" onconfirm={() => {}} />
    <ArmedButton label="Delete" confirmLabel="Really delete?" onconfirm={() => {}} />
    <ArmedButton label="Discard" confirmLabel="Discard" onconfirm={() => {}} disabled />
  </div>
  <div class="row">
    <div class="tb-right">
      <ArmedButton label="Archive" confirmLabel="Confirm" onconfirm={() => {}}>
        {#snippet icon()}<Icon name="trash" />{/snippet}
      </ArmedButton>
    </div>
  </div>
  <div class="row">
    <span class="lbl"><DecodeText text="feature/parity-scenes" /></span>
    <span class="lbl"><DecodeText text="Reviewing changes" animate={false} /></span>
  </div>
</div>

<style>
  .wrap { padding: 12px; display: flex; flex-direction: column; gap: 10px; }
  .row { display: flex; align-items: center; gap: 8px; }
  .lbl { font-size: 12px; color: var(--muted); }
</style>
