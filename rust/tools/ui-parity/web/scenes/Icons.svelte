<script lang="ts">
  import Icon from '../../../../../frontend/src/components/Icon.svelte'
  import LucideIcon from '../../../../../frontend/src/components/LucideIcon.svelte'
  import ProviderIcon from '../../../../../frontend/src/components/ProviderIcon.svelte'
  import { icons, type IconName } from '../../../../../frontend/src/lib/icons'
  import { lucideSuggested } from '../../../../../frontend/src/lib/lucide-icons'
  import { fileIconClass } from '../../../../../frontend/src/lib/file-icons'

  let { variant }: { variant: string } = $props()
  const appNames = Object.keys(icons) as IconName[]
  const files = [
    'main.rs', 'app.ts', 'index.js', 'package.json', '.env', 'README.md', 'Cargo.toml', 'go.mod',
    'style.scss', 'index.html', 'logo.png', 'Dockerfile', 'Makefile', 'tool.py', 'App.svelte',
    'data.json', 'notes.txt', 'unknown.zzz', 'deploy.yml', 'schema.sql',
  ]
  const providers = ['cursor', 'claude', 'codex', 'ollama', 'compatible']
</script>

<div class="grid" class:wide={variant === 'app'}>
  {#if variant === 'app'}
    {#each appNames as n}<span class="cell"><Icon name={n} /></span>{/each}
  {:else if variant === 'lucide'}
    {#each lucideSuggested as e}<span class="cell"><LucideIcon name={e.name} /></span>{/each}
  {:else if variant === 'provider'}
    {#each providers as p}<span class="cell prov"><ProviderIcon provider={p} /></span>{/each}
  {:else if variant.startsWith('file')}
    {#each files as f}<span class="cell tab-chip"><span class="fico {fileIconClass(f)}"></span></span>{/each}
  {/if}
</div>

<style>
  .grid { display: grid; grid-template-columns: repeat(16, 20px); gap: 4px; padding: 8px; color: var(--text); }
  .grid.wide { grid-template-columns: repeat(21, 20px); }
  .cell { width: 20px; height: 20px; display: grid; place-items: center; }
  .cell :global(svg) { width: 16px; height: 16px; }
  .cell.prov :global(svg) { width: 14px; height: 14px; }
</style>
