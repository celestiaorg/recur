<script lang="ts">
  import AppProviderIcon from "../../../demo/src/components/ProviderIcon.svelte";

  let { provider = "" }: { provider?: string } = $props();
</script>

<AppProviderIcon {provider} />
