<script lang="ts">
  import Select from './Select.svelte'
  import Icon from './Icon.svelte'
  import type { LoopInfo, PolicyInfo } from '../lib/loops'
  import { loopSelectOptions, policySelectOptions } from '../lib/loops'

  let {
    loopId = $bindable(''),
    policyId = $bindable(''),
    loops = [],
    policies = [],
    pinned = false,
    ghost = true,
    up = true,
    showTip = false,
    onLoopChange,
    onPolicyChange,
  }: {
    loopId?: string
    policyId?: string
    loops?: LoopInfo[]
    policies?: PolicyInfo[]
    pinned?: boolean
    ghost?: boolean
    up?: boolean
    showTip?: boolean
    onLoopChange?: (id: string) => void
    onPolicyChange?: (id: string) => void
  } = $props()

  let tipOpen = $state(false)
  const loopOpts = $derived(loopSelectOptions(loops))
  const policyOpts = $derived(policySelectOptions(policies))

  function pickLoop(v: string) {
    loopId = v
    onLoopChange?.(v)
  }
  function pickPolicy(v: string) {
    policyId = v
    onPolicyChange?.(v)
  }
</script>

<div class="nt-autonomy">
  <Select
    {ghost}
    {up}
    label="Loop"
    options={loopOpts}
    value={loopId}
    disabled={pinned || loopOpts.length === 0}
    onchange={pickLoop}
  />
  <Select
    {ghost}
    {up}
    label="Policy"
    options={policyOpts}
    value={policyId}
    disabled={pinned || policyOpts.length === 0}
    onchange={pickPolicy}
  >
    {#snippet icon()}
      <Icon name="shield" />
    {/snippet}
  </Select>
  {#if showTip}
    <div class="nt-tip">
      <button
        type="button"
        class="nt-help"
        class:on={tipOpen}
        aria-label="What is a policy?"
        aria-expanded={tipOpen}
        aria-controls="loop-policy-tip"
        onclick={() => (tipOpen = !tipOpen)}
      >
        <Icon name="info" />
      </button>
      {#if tipOpen}
        <div class="nt-tip-pop" id="loop-policy-tip" role="tooltip">
          <b>Policy</b> is how hard this loop pushes before it asks you.
          <ul>
            <li><b>Cautious</b>: every finding; escalate after 2 rounds</li>
            <li><b>Balanced</b>: medium and up; escalate after 4</li>
            <li><b>Aggressive</b>: ship-blockers only; proceed after 6</li>
          </ul>
        </div>
      {/if}
    </div>
  {/if}
</div>
