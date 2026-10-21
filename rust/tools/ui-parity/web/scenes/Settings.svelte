<script lang="ts">
  import SettingsGroups from '../../../../../frontend/src/components/SettingsGroups.svelte'
  import SettingsGroup from '../../../../../frontend/src/components/SettingsGroup.svelte'
</script>

<div class="wrap">
  <SettingsGroups>
    <SettingsGroup title="Appearance" blurb="Theme and density for every window.">
      <div class="body">Body content</div>
    </SettingsGroup>
    <SettingsGroup title="Agent defaults" override={true}>
      {#snippet blurbContent()}Applied to new chats. Set <code>model</code> per project.{/snippet}
      <div class="body">Overridden here</div>
    </SettingsGroup>
    <SettingsGroup title="Project override" blurb="Uses the global settings." override={false}>
      <div class="body">Hidden</div>
    </SettingsGroup>
    <SettingsGroup title="Locked" override={false} overrideDisabled />
    <SettingsGroup title="Editor">
      <SettingsGroup class="set-subsec-nested" title="Indent by language" blurb="Auto uses common conventions.">
        <div class="body">Go · tabs</div>
      </SettingsGroup>
    </SettingsGroup>
  </SettingsGroups>
</div>

<style>
  .wrap { padding: 12px; width: 420px; }
  .body { font-size: 12px; color: var(--muted); }
</style>
