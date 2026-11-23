<script lang="ts">
  import { tick } from "svelte";
  import { fade } from "svelte/transition";
  import { MediaQuery } from "svelte/reactivity";
  import ProductScreenshot from "./ProductScreenshot.svelte";
  import { screens, type Screen } from "../site.svelte";

  // Clickable areas over the captures, in a 2000×1111 space. Every capture has
  // the same sidebar, so those areas hold on all of them. Check these after a
  // recapture that moves anything.
  type Spot = [to: Screen, label: string, x: number, y: number, w: number, h: number];
  const row = (to: Screen, label: string, y: number, h: number): Spot => [to, label, 7, y, 374, h];
  const sidebar: Spot[] = [
    row("inbox", "Inbox", 64, 42),
    row("issues", "Issues", 108, 42),
    row("pull-request", "Pull requests", 153, 42),
    row("schedules", "Schedules", 197, 42),
    row("insights", "Insights", 242, 42),
    row("loop", "Passkey sign-in", 547, 38),
    row("workspace", "Session refresh", 702, 38),
    row("uncommitted", "Idempotency keys", 821, 38),
    row("background", "Webhook retries", 860, 38),
  ];
  const back: Spot = ["inbox", "Back to Inbox", 180, 8, 80, 36];
  const viewPr: Spot = ["workspace-pr", "View PR #482", 1638, 8, 158, 36];
  const icon = (to: Screen, label: string, x: number): Spot => [to, label, x - 19, 58, 38, 38];
  const listed = (to: Screen, label: string, y: number): Spot => [to, label, 890, y, 471, 71];
  const spots: Partial<Record<Screen, Spot[]>> = {
    loop: [back],
    background: [back],
    uncommitted: [back],
    workspace: [
      back,
      viewPr,
      icon("changes", "Review", 1512),
      icon("workspace-pr", "Pull request", 1554),
      ["workspace-pr", "Review chat", 602, 104, 140, 44],
    ],
    changes: [
      back,
      viewPr,
      icon("workspace", "Editor", 1465),
      icon("workspace-pr", "Pull request", 1549),
    ],
    "workspace-pr": [
      back,
      icon("workspace", "Editor", 1465),
      icon("changes", "Review", 1507),
      ["workspace", "Session refresh chat", 390, 104, 208, 44],
      ["changes", "Files changed", 1274, 104, 196, 44],
    ],
    inbox: [
      listed("loop", "Open Passkey sign-in", 247),
      listed("workspace", "Open Session refresh", 575),
      listed("background", "Open Webhook retries", 648),
      listed("uncommitted", "Open Idempotency keys", 721),
    ],
    issues: [
      ["background", "PAY-88: view workspace", 1786, 160, 192, 39],
      ["uncommitted", "PAY-91: view workspace", 1786, 451, 192, 39],
      ["workspace", "#471: view workspace", 1786, 691, 192, 39],
    ],
  };

  const wide = new MediaQuery("min-width: 900px");
  const fadeMs = matchMedia("(prefers-reduced-motion: reduce)").matches ? 0 : 140;
  const pills = screens.filter((s) =>
    ["loop", "inbox", "workspace-pr", "insights"].includes(s.screen),
  );

  let screen = $state.raw(screens[0]);
  let shown = $state.raw(screens[0]);
  let touched = $state(false);
  let hint = $state(0);
  let frame = $state<HTMLElement>();

  const load = (to: Screen) => {
    const image = new Image();
    image.src = `/screenshots/${to}.png`;
    return image.decode().catch(() => {});
  };
  // The capture changes once it has decoded, so it fades in whole.
  async function show(to: Screen) {
    touched = true;
    const item = screens.find((s) => s.screen === to)!;
    screen = item;
    await load(to);
    if (screen !== item) return;
    const inside = frame?.contains(document.activeElement);
    shown = item;
    await tick();
    if (inside && !frame?.contains(document.activeElement)) frame?.focus();
  }
</script>

{#if wide.current}
  <p class="hint">Real screenshots. Click around.</p>
{:else}
  <div class="pills" role="group" aria-label="Recur screens">
    {#each pills as item (item.screen)}<button
        type="button"
        aria-pressed={screen === item}
        onclick={() => show(item.screen)}>{item.label}</button
      >{/each}
  </div>
{/if}

<div
  class="frame"
  bind:this={frame}
  tabindex="-1"
  role="group"
  aria-label="Recur: {shown.label}"
>
  {#key shown}<div
      class="layer"
      in:fade={{ duration: fadeMs }}
      out:fade={{ delay: fadeMs, duration: 0 }}
    >
      {#if wide.current}
        <img
          src="/screenshots/{shown.screen}.png"
          alt={shown.caption}
          width="1440"
          height="800"
        />
      {:else}
        <ProductScreenshot screen={shown.screen} eager caption={shown.caption} />
      {/if}
    </div>{/key}
  {#if wide.current}
    <!-- A click that misses shows where the clickable areas are. -->
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div
      class="spots"
      class:fresh={!touched}
      onclick={(event) => {
        if (event.target === event.currentTarget) hint++;
      }}
    >
      {#each [...sidebar, ...(spots[shown.screen] ?? [])] as [to, label, x, y, w, h] (label)}<button
          type="button"
          aria-label={label}
          aria-current={shown.screen === to ? "page" : undefined}
          style:left="{x / 20}%"
          style:top="{y / 11.11}%"
          style:width="{w / 20}%"
          style:height="{h / 11.11}%"
          onpointerenter={() => load(to)}
          onclick={() => show(to)}
          >{#key hint}{#if hint}<i></i>{/if}{/key}</button
        >{/each}
    </div>
  {/if}
</div>
<div class="caption" aria-live="polite">
  {#each screens as item (item.screen)}<p class:on={screen === item}>
      {item.caption}
    </p>{/each}
</div>

<style>
  .hint {
    margin-bottom: 14px;
    font-size: 13px;
    color: var(--faint);
  }
  .pills {
    display: flex;
    justify-content: center;
    gap: 2px;
    width: fit-content;
    max-width: 100%;
    margin: 0 auto 18px;
    padding: 4px;
    overflow-x: auto;
    border: 1px solid var(--line);
    border-radius: 999px;
    background: #111112cc;
  }
  .pills button {
    flex: none;
    padding: 6px 12px;
    border-radius: 999px;
    font-size: 13px;
    color: var(--muted);
  }
  .pills button[aria-pressed="true"] {
    background: var(--text);
    color: var(--bg);
  }
  .frame {
    position: relative;
    display: grid;
    overflow: hidden;
    border: 1px solid var(--line-bright);
    border-radius: 12px;
    box-shadow:
      0 0 0 1px #0008,
      0 40px 80px -20px #000;
  }
  .layer {
    grid-area: 1 / 1;
  }
  .layer img {
    display: block;
    width: 100%;
    height: auto;
  }
  .spots {
    position: absolute;
    inset: 0;
  }
  .spots button {
    position: absolute;
    border-radius: 6px;
    cursor: pointer;
  }
  .spots button:hover {
    background: #ffffff0f;
  }
  .spots :global(:focus-visible) {
    outline-offset: -2px;
  }
  .spots i {
    position: absolute;
    inset: 0;
    border-radius: inherit;
    animation: spot 1s ease-out forwards;
    pointer-events: none;
  }
  .fresh button:first-child::after {
    content: "";
    position: absolute;
    inset: 0;
    border-radius: inherit;
    animation: spot 1.8s ease-out 1s 3;
  }
  @keyframes spot {
    0% {
      box-shadow: inset 0 0 0 1.5px #e07a5f00;
    }
    30% {
      background: #e07a5f26;
      box-shadow: inset 0 0 0 1.5px #e07a5f;
    }
    100% {
      box-shadow: inset 0 0 0 1.5px #e07a5f00;
    }
  }
  .caption {
    display: grid;
    max-width: 640px;
    margin: 18px auto 0;
    font-size: 13.5px;
    color: var(--muted);
  }
  /* Every caption sits in the same cell, so the page never changes height. */
  .caption p {
    grid-area: 1 / 1;
    visibility: hidden;
  }
  .caption .on {
    visibility: visible;
  }
</style>
