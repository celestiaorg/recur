<script lang="ts">
  import AppProviderIcon from "../../../../frontend/src/components/ProviderIcon.svelte";

  let { provider = "" }: { provider?: string } = $props();
</script>

<AppProviderIcon {provider} />
