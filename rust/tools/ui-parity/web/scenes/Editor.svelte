<script lang="ts">
  import { onMount } from 'svelte'
  import CodeEditor from '../../../../../frontend/src/components/CodeEditor.svelte'
  import { ensureTextMateBootstrap } from '../../../../../frontend/src/lib/tm-bootstrap'
  import { applyEditorTheme, setEditorPrefs } from '../../../../../frontend/src/lib/stores'

  let { variant }: { variant: string } = $props()

  // Same document and editor theme as `recur_editor::gallery`.
  const doc = `package main

import (
	"fmt"
	"strings"
)

// greet joins names for the banner.
func greet(names []string) string {
	return "hello, " + strings.Join(names, ", ")
}

func main() {
	fmt.Println(greet([]string{"recur", "gpui"}))
}
`
  let booted = $state(false)
  onMount(async () => {
    await ensureTextMateBootstrap()
    applyEditorTheme(variant === 'paper' ? 'paper' : 'recur')
    if (variant === 'vim') setEditorPrefs({ keymap: 'vim' })
    booted = true
  })
</script>

<div class="editor-scene">
  {#if booted}
    <CodeEditor {doc} path="main.go" readOnly={false} />
  {/if}
</div>

<style>
  .editor-scene { position: absolute; inset: 0; display: flex; flex-direction: column; }
</style>
