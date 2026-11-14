<script lang="ts">
  import { fade } from "svelte/transition";
  import ProductScreenshot from "./lib/components/ProductScreenshot.svelte";
  import LoopFlow from "./lib/components/LoopFlow.svelte";
  import ProviderIcon from "./lib/components/ProviderIcon.svelte";
  import {
    docs,
    downloadLabel,
    downloads,
    github,
    icon,
    keys,
    linuxBuilds,
    linuxFormats,
    loopExample,
    loops,
    modes,
    policies,
    screens,
    telegram,
    visitor,
    wordmark,
    x,
  } from "./lib/site.svelte";

  let screen = $state.raw(screens[0]);
  let mode = $state.raw(modes[2]);
  let playing = $state(true);

  // The tab moves at once; the capture follows once it has decoded, so it
  // fades in whole over the one it replaces.
  const fadeMs = matchMedia("(prefers-reduced-motion: reduce)").matches ? 0 : 400;
  let shown = $state.raw(screens[0]);
  async function show(item: (typeof screens)[number]) {
    screen = item;
    const image = new Image();
    image.src = `/screenshots/${item.screen}.png`;
    await image.decode().catch(() => {});
    if (screen === item) shown = item;
  }

  // The tour advances on its own until the visitor takes over. The fill that
  // sweeps the active pill times each stop, so pausing it pauses the tour.
  let auto = $state(fadeMs > 0);
  let offscreen = $state(false);
  function watchStage(node: HTMLElement) {
    const obs = new IntersectionObserver(([e]) => {
      offscreen = !e.isIntersecting;
    });
    obs.observe(node);
    return () => obs.disconnect();
  }

  // The active tab's pill slides between tabs: the edge facing the new tab
  // leads and the other catches up, so it stretches on the way.
  let tabs = $state<HTMLElement>();
  let tabsWidth = $state(0);
  let pill = $state<{ left: number; right: number; forward: boolean }>();
  let pillLeft = 0;
  $effect(() => {
    const tab = tabs?.children[screens.indexOf(screen)] as HTMLElement;
    if (!tab || !tabsWidth) return;
    const left = tab.offsetLeft;
    pill = {
      left,
      right: tabsWidth - left - tab.offsetWidth,
      forward: left >= pillLeft,
    };
    pillLeft = left;
  });

  const schedules = [
    ["Nightly review & fix", "0 2 * * 1-5", "Autopilot · Review & fix · Balanced"],
    ["Weekly security pass", "0 9 * * 1", "Autopilot · Security review & fix"],
    ["Morning code review", "@daily", "Review"],
    ["Refresh dev fixtures", "@reboot", "Agent"],
    ["Release notes draft", "once Oct 1 09:00", "Agent"],
  ];
  const agents = [
    ["claude", "Claude Code"],
    ["cursor", "Cursor"],
    ["codex", "Codex"],
  ];
  const local = [
    "Workspaces are git worktrees on your disk.",
    "Model calls go straight to the provider you picked.",
    "GitHub goes through your own gh login.",
  ];
</script>

{#snippet cta(id: string)}
  <div class="bn-actions">
    <a class="bn-button" href={visitor.download}
      >{downloadLabel} ↓</a
    ><a class="bn-button bn-ghost" href={docs}>Read the docs</a>
  </div>
  <p class="bn-fine">
    macOS 12+ and Linux (x64, arm64).
    <button type="button" popovertarget={id} style:anchor-name="--{id}"
      >Other downloads ▾</button
    >
  </p>
  <div {id} class="bn-downloads" popover="auto" style:position-anchor="--{id}">
    <dl>
      <div>
        <dt>macOS</dt>
        <dd>
          <a href="{downloads}/Recur-macos-universal.dmg">.dmg</a>
          <span>12+, Apple Silicon and Intel</span>
        </dd>
      </div>
      {#each linuxBuilds as build (build.arch)}<div>
          <dt>Linux {build.label}</dt>
          <dd>
            {#each linuxFormats as format (format)}<a
                href="{downloads}/Recur-linux-{build.arch}.{format}"
                >.{format}</a
              >{/each}
          </dd>
        </div>{/each}
    </dl>
    <p>
      Linux needs GTK 4 and WebKitGTK 6.0 (Ubuntu 24.04+, Debian 13+). See the
      <a href="{docs}getting-started/install">install guide</a>.
    </p>
  </div>
{/snippet}

{#snippet head(title: string, copy: string, path: string)}
  <header class="bn-head">
    <h3>{title}</h3>
    <p>{copy} <a href="{docs}{path}" aria-label="Docs: {title}">Docs →</a></p>
  </header>
{/snippet}

<div class="bn" id="top">
  <a class="bn-skip" href="#main">Skip to content</a>
  <header class="bn-nav">
    <a href="#top" aria-label="Recur home"
      ><img src={wordmark} alt="Recur" width="720" height="172" /></a
    >
    <nav aria-label="Main navigation">
      <a href={docs}>Docs</a><a href={github}>GitHub</a><a
        class="bn-social"
        href={x}>X</a
      ><a class="bn-social" href={telegram}>Telegram</a><a
        class="bn-button bn-small"
        href={visitor.download}>Download</a
      >
    </nav>
  </header>

  <main id="main">
    <section class="bn-hero">
      <h1>Agents that don’t stop <em>until it’s right.</em></h1>
      <p class="bn-lede">
        Recur runs Claude Code, Cursor, or Codex through workflows where a team
        of agents plans, builds, and reviews until the work clears your bar.
      </p>
      {@render cta("bn-hero-downloads")}

      <!-- svelte-ignore a11y_no_static_element_interactions (Only ends the tour; the tabs are the controls.) -->
      <div
        class="bn-stage"
        onpointerdown={() => (auto = false)}
        {@attach watchStage}
      >
        <div class="bn-tabs">
          <div
            role="group"
            aria-label="Recur screens"
            bind:this={tabs}
            bind:clientWidth={tabsWidth}
          >
            {#each screens as item (item.screen)}<button
                type="button"
                aria-pressed={screen === item}
                onclick={() => {
                  auto = false;
                  show(item);
                }}>{item.label}</button
              >{/each}
            {#if pill}<span
                aria-hidden="true"
                style:left="{pill.left}px"
                style:right="{pill.right}px"
                style:transition-duration={pill.forward
                  ? "0.45s, 0.25s"
                  : "0.25s, 0.45s"}
                >{#if auto}{#key screen}<i
                      class:paused={offscreen}
                      onanimationend={() =>
                        show(
                          screens[(screens.indexOf(screen) + 1) % screens.length],
                        )}
                    ></i>{/key}{/if}</span
              >{/if}
          </div>
        </div>
        <div class="bn-window">
          {#key shown}<div
              in:fade={{ duration: fadeMs }}
              out:fade={{ delay: fadeMs, duration: 0 }}
            >
              <ProductScreenshot
                screen={shown.screen}
                eager
                caption={shown.caption}
              />
            </div>{/key}
        </div>
        <div class="bn-caption" aria-live={auto ? "off" : "polite"}>
          {#each screens as item (item.screen)}<p class:on={screen === item}>
              {item.caption}
            </p>{/each}
        </div>
      </div>
    </section>

    <section class="bn-features" aria-labelledby="bn-features">
      <h2 id="bn-features">Agent work should <em>arrive finished.</em></h2>
      <div class="bn-grid">
        <article class="bn-card bn-12">
          <div class="bn-row">
            {@render head(
              "A team of agents instead of one.",
              "A planner scopes it, a coder builds it, a reviewer tears into it, and they go round until it’s right or they need you.",
              "concepts/workflows",
            )}
            <button
              type="button"
              class="bn-play"
              aria-pressed={playing}
              onclick={() => (playing = !playing)}
              >{playing ? "Ⅱ Pause" : "▶ Play"}</button
            >
          </div>
          <!-- svelte-ignore a11y_no_noninteractive_tabindex (Keyboard users need to scroll this wide graph.) -->
          <div
            class="bn-scroll"
            tabindex="0"
            role="region"
            aria-label="Animated workflow graph"
          >
            <div class="bn-graph" class:paused={!playing}>
              <LoopFlow {playing} />
            </div>
          </div>
        </article>

        <article class="bn-card bn-7">
          {@render head(
            "Start one with a slash command.",
            "The shipped workflows. Type one in any chat.",
            "concepts/workflows",
          )}
          <ul class="bn-commands">
            {#each loops as [command, text] (command)}<li>
                <code>{command}</code><span>{text}</span>
              </li>{/each}
          </ul>
        </article>

        <article class="bn-card bn-5">
          {@render head(
            "Pick a mode for each chat.",
            "Every chat picks its own agent and how far it’s allowed to go.",
            "concepts/modes",
          )}
          <div class="bn-modes" role="group" aria-label="Chat modes">
            {#each modes as item (item.name)}<button
                type="button"
                aria-pressed={mode === item}
                onclick={() => (mode = item)}
                >{item.name}<span class="bn-{item.access}">{item.access}</span
                ></button
              >{/each}
          </div>
          <div class="bn-prompt" aria-live="polite">
            {#each modes as item (item.name)}<div class:on={mode === item}>
                <strong>{item.title}</strong>
                <p>{item.body}</p>
                <blockquote>{item.example}</blockquote>
                <span>{item.outcome}</span>
              </div>{/each}
          </div>
        </article>

        <article class="bn-card bn-5">
          {@render head(
            "A policy sets the bar.",
            "What blocks, whether you approve the plan, and when to stop.",
            "concepts/workflows",
          )}
          <div class="bn-scroll">
            <table>
              <thead>
                <tr>
                  {#each ["Policy", "Blocks on", "Plan approval", "Round cap"] as h (h)}<th scope="col">{h}</th>{/each}
                </tr>
              </thead>
              <tbody>
                {#each policies as [name, ...cells] (name)}<tr
                    ><th scope="row">{name}</th>{#each cells as cell (cell)}<td
                        >{cell}</td
                      >{/each}</tr
                  >{/each}
              </tbody>
            </table>
          </div>
        </article>

        <article class="bn-card bn-7">
          {@render head(
            "Put the chores on a schedule.",
            "Any prompt or workflow can run on its own clock, each time in a fresh workspace.",
            "concepts/schedules",
          )}
          <div class="bn-scroll">
            <table class="bn-cron">
              <thead>
                <tr>
                  {#each ["Name", "Cadence", "Mode"] as h (h)}<th scope="col">{h}</th>{/each}
                </tr>
              </thead>
              <tbody>
                {#each schedules as [name, cadence, how] (name)}<tr
                    ><th scope="row">{name}</th><td><code>{cadence}</code></td
                    ><td>{how}</td></tr
                  >{/each}
              </tbody>
            </table>
          </div>
        </article>

        <article class="bn-card bn-6 bn-shot">
          {@render head(
            "One inbox, every agent.",
            "Every workspace across your projects: what needs you, what’s in progress, and what’s ready to pick up.",
            "concepts/inbox",
          )}
          <div class="bn-crop">
            <div><ProductScreenshot screen="inbox" /></div>
          </div>
        </article>

        <article class="bn-card bn-6 bn-shot">
          {@render head(
            "Every PR, one place.",
            "Every open PR across your projects, with agent findings right next to human comments.",
            "concepts/pull-requests",
          )}
          <div class="bn-crop">
            <div><ProductScreenshot screen="pull-request" /></div>
          </div>
        </article>

        <article class="bn-card bn-wide">
          {@render head(
            "Runs the agents you already use.",
            "Plus OpenAI-compatible: OpenAI, OpenRouter, Groq, or a custom URL such as Ollama.",
            "reference/agents",
          )}
          <ul class="bn-agents">
            {#each agents as [provider, name] (provider)}<li>
                <ProviderIcon {provider} />{name}
              </li>{/each}
          </ul>
        </article>

        <article class="bn-card bn-5">
          {@render head(
            "Keep your hands on the keys.",
            "Every workspace, file, chat, and command is a chord away, and the editor speaks Vim and Emacs.",
            "reference/shortcuts",
          )}
          <ul class="bn-keys">
            {#each keys as [chords, label] (label)}<li>
                {label}<span
                  >{#each chords as chord (chord)}<kbd>{chord}</kbd>{/each}</span
                >
              </li>{/each}
          </ul>
        </article>

        <article class="bn-card bn-7">
          {@render head(
            "Your process, written down.",
            "Workflows and roles live in plain TOML bundles. Copy one, swap a role or a prompt, and reload without restarting.",
            "bundles/workflows",
          )}
          <!-- svelte-ignore a11y_no_noninteractive_tabindex (Keyboard users need to scroll this code sample.) -->
          <pre
            class="bn-toml"
            tabindex="0"
            aria-label="Example custom workflow in a user bundle">{#each loopExample as token, i (i)}<span
                class="t-{token.kind}">{token.text}</span
              >{/each}</pre>
        </article>
      </div>
    </section>

    <section class="bn-close" id="download">
      <div>
        <h2>Local by<br />design.</h2>
        <ul>
          {#each local as fact (fact)}<li>{fact}</li>{/each}
        </ul>
        {@render cta("bn-close-downloads")}
      </div>
      <img src={icon} alt="" width="204" height="204" />
    </section>
  </main>

  <footer class="bn-footer">
    <img src={wordmark} alt="Recur" width="720" height="172" />
    <nav aria-label="Footer">
      <a href={docs}>Docs</a><a href={github}>GitHub</a><a href={x}>X</a><a
        href={telegram}>Telegram</a
      >
    </nav>
  </footer>
</div>

<style>
  .bn {
    --bg: #09090a;
    --pane: #111112;
    --pane-2: #1a1a1c;
    --line: #232326;
    --line-soft: #1c1c1f;
    --line-bright: #35353a;
    --text: #ededeb;
    --muted: #a3a19c;
    --faint: #7d7b77;
    --amber: #e07a5f;
    --mono: "IBM Plex Mono", ui-monospace, monospace;
    min-height: 100vh;
    padding: 0 24px 72px;
    background:
      radial-gradient(60% 420px at 50% 0, #e07a5f1f, transparent), var(--bg);
    color: var(--text);
    font: 15px/1.55 "Inter", system-ui, sans-serif;
    color-scheme: dark;
  }
  .bn > *,
  .bn main > * {
    max-width: 1200px;
    margin-inline: auto;
  }
  .bn :global(:focus-visible) {
    outline: 2px solid var(--cyan);
    outline-offset: 3px;
    border-radius: 4px;
  }
  .bn em {
    font-style: normal;
    color: var(--amber);
  }
  .bn code,
  .bn pre {
    font-family: var(--mono);
  }

  .bn-skip {
    position: absolute;
    left: 16px;
    top: -80px;
    z-index: 50;
    padding: 10px;
    background: var(--text);
    color: var(--bg);
  }
  .bn-skip:focus {
    top: 10px;
  }
  .bn-nav,
  .bn-footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    height: 72px;
  }
  .bn-nav img,
  .bn-footer img {
    width: 100px;
    height: auto;
  }
  .bn nav {
    display: flex;
    align-items: center;
    gap: 22px;
    font-size: 13px;
    color: var(--muted);
  }
  .bn nav a:hover,
  .bn-fine button:hover,
  .bn-head a:hover {
    color: var(--text);
  }

  .bn-button {
    display: inline-flex;
    align-items: center;
    gap: 10px;
    padding: 12px 20px;
    border: 1px solid var(--text);
    border-radius: 999px;
    background: var(--text);
    color: var(--bg);
    font-size: 14px;
    font-weight: 600;
    transition:
      background-color 0.15s,
      border-color 0.15s;
  }
  .bn-button:hover {
    background: #fff;
    border-color: #fff;
  }
  .bn-ghost {
    background: transparent;
    border-color: var(--line-bright);
    color: var(--text);
  }
  .bn-ghost:hover {
    background: var(--pane-2);
    border-color: var(--faint);
  }
  .bn-nav .bn-small {
    padding: 7px 14px;
    font-size: 13px;
    color: var(--bg);
  }
  .bn-actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 10px;
  }
  .bn-fine {
    margin-top: 14px;
    font-size: 12.5px;
    color: var(--faint);
  }
  .bn-fine button,
  .bn-head a {
    color: var(--muted);
    text-decoration: underline;
    text-decoration-color: var(--line-bright);
    text-underline-offset: 3px;
  }
  /* The popover sits in the top layer but still inherits the closing card's
     inverted palette, so it sets its own. */
  .bn-downloads {
    --text: #ededeb;
    --muted: #a3a19c;
    --line-bright: #35353a;
    width: min(380px, calc(100vw - 32px));
    padding: 16px 18px;
    border: 1px solid var(--line-bright);
    border-radius: 12px;
    background: #111112;
    color: var(--text);
    text-align: left;
    box-shadow: 0 12px 32px #0008;
  }
  @supports (anchor-name: --a) {
    .bn-downloads {
      inset: auto;
      top: anchor(bottom);
      left: anchor(left);
      margin: 8px 0;
      position-try-fallbacks: flip-block, flip-inline;
    }
  }
  .bn-downloads dl > div {
    display: grid;
    grid-template-columns: 96px 1fr;
    align-items: center;
    padding: 8px 0;
    border-bottom: 1px solid #232326;
  }
  .bn-downloads dt {
    font-size: 13px;
    font-weight: 600;
  }
  .bn-downloads dd {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 8px;
  }
  .bn-downloads dd a {
    padding: 4px 9px;
    border: 1px solid var(--line-bright);
    border-radius: 6px;
    font: 12px var(--mono);
  }
  .bn-downloads dd a:hover {
    border-color: var(--amber);
    color: var(--amber);
  }
  .bn-downloads span,
  .bn-downloads p {
    font-size: 12px;
    color: var(--muted);
  }
  .bn-downloads p {
    margin-top: 10px;
    line-height: 1.5;
  }
  .bn-downloads p a {
    color: var(--text);
    text-decoration: underline;
  }

  .bn-hero {
    text-align: center;
    padding-top: 56px;
  }
  .bn h1,
  .bn h2 {
    font-weight: 600;
    letter-spacing: -0.04em;
    text-wrap: balance;
  }
  .bn h1 em {
    display: block;
  }
  .bn h1 {
    margin: 22px auto 18px;
    font-size: clamp(40px, 6.4vw, 76px);
    line-height: 1.02;
  }
  .bn h2 {
    font-size: clamp(30px, 4vw, 46px);
    line-height: 1.08;
  }
  .bn-lede {
    max-width: 640px;
    margin: 0 auto 28px;
    font-size: clamp(16px, 1.5vw, 18px);
    color: var(--muted);
    text-wrap: balance;
  }

  .bn-stage {
    position: relative;
    margin-top: 56px;
  }
  .bn-stage::before {
    content: "";
    position: absolute;
    inset: 40px 12% 45%;
    z-index: 0;
    background: #e07a5f;
    opacity: 0.22;
    filter: blur(110px);
  }
  .bn-stage > * {
    position: relative;
  }
  .bn-tabs {
    width: fit-content;
    max-width: 100%;
    margin: 0 auto 18px;
    padding: 4px;
    overflow-x: auto;
    scrollbar-width: none;
    border: 1px solid var(--line);
    border-radius: 999px;
    background: #111112cc;
  }
  .bn-tabs > div {
    position: relative;
    display: flex;
    gap: 2px;
    width: max-content;
  }
  .bn-tabs span {
    position: absolute;
    top: 0;
    bottom: 0;
    border-radius: 999px;
    background: var(--text);
    transition-property: left, right;
    transition-timing-function: cubic-bezier(0.3, 1, 0.3, 1);
    overflow: hidden;
  }
  .bn-tabs i {
    position: absolute;
    inset: 0;
    background: color-mix(in srgb, var(--amber) 45%, transparent);
    transform: scaleX(0);
    transform-origin: left;
    animation: bn-tour 6s linear forwards;
  }
  .bn-tabs i.paused,
  .bn-stage:hover i,
  .bn-stage:focus-within i {
    animation-play-state: paused;
  }
  @keyframes bn-tour {
    to {
      transform: scaleX(1);
    }
  }
  .bn-tabs button {
    position: relative;
    z-index: 1;
    padding: 6px 14px;
    border-radius: 999px;
    font-size: 13px;
    color: var(--muted);
    transition: color 0.2s;
  }
  .bn-tabs button:hover {
    color: var(--text);
  }
  .bn-tabs button[aria-pressed="true"] {
    color: var(--bg);
  }
  /* Screenshots open full size on click; say so on hover, and always on touch. */
  .bn-window::after,
  .bn-shot::after {
    content: "View full size";
    position: absolute;
    right: 14px;
    bottom: 14px;
    z-index: 1;
    padding: 7px 13px 7px 33px;
    border: 1px solid var(--line-bright);
    border-radius: 999px;
    background: #09090ae6
      url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='%23ededeb' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'%3E%3Ccircle cx='11' cy='11' r='8'/%3E%3Cpath d='m21 21-4.3-4.3M11 8v6M8 11h6'/%3E%3C/svg%3E")
      12px 50% / 14px no-repeat;
    font-size: 12.5px;
    font-weight: 500;
    pointer-events: none;
    opacity: 0;
    transition: opacity 0.15s;
  }
  .bn-window:hover::after,
  .bn-window:has(:focus-visible)::after,
  .bn-shot:has(.bn-crop:hover)::after,
  .bn-shot:has(:global(.capture-trigger:focus-visible))::after {
    opacity: 1;
  }
  @media (hover: none) {
    .bn-window::after,
    .bn-shot::after {
      opacity: 1;
    }
  }
  .bn-window,
  .bn-crop,
  .bn-window :global(.capture-trigger),
  .bn-crop :global(.capture-trigger) {
    cursor: pointer;
  }
  .bn-window {
    position: relative;
    display: grid;
    overflow: hidden;
    border: 1px solid var(--line-bright);
    border-radius: 12px;
    box-shadow:
      0 0 0 1px #0008,
      0 40px 80px -20px #000;
  }
  .bn-window > div {
    grid-area: 1 / 1;
  }
  /* Every caption sits in the same cell, so the tour never changes height. */
  .bn-caption {
    display: grid;
    max-width: 640px;
    margin: 18px auto 0;
    font-size: 13.5px;
    color: var(--muted);
  }
  .bn-caption p {
    grid-area: 1 / 1;
    visibility: hidden;
  }
  .bn-caption .on {
    visibility: visible;
  }

  .bn-features {
    padding-top: 88px;
  }
  .bn-features > h2 {
    margin-bottom: 36px;
    text-align: center;
  }
  .bn-grid {
    display: grid;
    grid-template-columns: repeat(12, minmax(0, 1fr));
    gap: 16px;
  }
  .bn-card {
    grid-column: span 12;
    display: flex;
    flex-direction: column;
    gap: 22px;
    min-width: 0;
    padding: 26px;
    overflow: hidden;
    border: 1px solid var(--line);
    border-radius: 16px;
    background: linear-gradient(#141416, var(--pane) 120px);
    transition:
      border-color 0.2s,
      transform 0.2s;
  }
  .bn-card:hover {
    border-color: var(--line-bright);
    transform: translateY(-2px);
  }
  .bn-head h3 {
    margin-bottom: 6px;
    font-size: 19px;
    font-weight: 600;
    letter-spacing: -0.02em;
  }
  .bn-head p {
    max-width: 560px;
    font-size: 14px;
    color: var(--muted);
  }
  .bn-head a {
    white-space: nowrap;
    font-size: 13px;
  }
  .bn-row {
    display: flex;
    align-items: start;
    justify-content: space-between;
    gap: 16px;
  }
  .bn-play {
    flex: none;
    padding: 5px 10px;
    border: 1px solid var(--line);
    border-radius: 999px;
    font: 11px var(--mono);
    color: var(--muted);
  }
  .bn-scroll {
    flex: 1;
    overflow-x: auto;
    container-type: inline-size;
  }
  /* The graph scrolls sideways on small screens; its caption stays in view. */
  .bn-graph :global(.lf-foot) {
    position: sticky;
    left: 0;
    width: 100cqw;
  }
  .bn-graph {
    min-width: 820px;
  }
  .bn-graph.paused :global(*) {
    animation-play-state: paused !important;
  }
  .bn-graph :global(.loop-flow) {
    padding: 0;
    border: 0;
    background: none;
  }

  .bn-commands,
  .bn-keys,
  .bn-agents,
  .bn-close ul {
    list-style: none;
  }
  /* Rows stretch so the shorter card of a pair fills its height. */
  .bn-commands {
    flex: 1;
    display: grid;
  }
  .bn-commands li {
    display: grid;
    grid-template-columns: 150px 1fr;
    gap: 6px 18px;
    align-items: baseline;
    padding: 11px 0;
    border-top: 1px solid var(--line-soft);
    font-size: 13.5px;
    color: var(--muted);
  }
  .bn-commands code,
  .bn-cron code {
    font-size: 12.5px;
    color: var(--amber);
    white-space: nowrap;
  }

  .bn table {
    width: 100%;
    height: 100%;
    border-collapse: collapse;
    font-size: 13px;
    white-space: nowrap;
  }
  .bn th,
  .bn td {
    padding: 11px 16px 11px 0;
    border-top: 1px solid var(--line-soft);
    text-align: left;
    font-weight: 400;
    color: var(--muted);
  }
  .bn thead th {
    padding-block: 0 9px;
    border: 0;
    font: 10.5px var(--mono);
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--faint);
  }
  .bn tbody th {
    font-weight: 500;
    color: var(--text);
  }

  .bn-modes {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .bn-modes button {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 6px 8px 6px 12px;
    border: 1px solid var(--line);
    border-radius: 999px;
    font-size: 13px;
    color: var(--muted);
  }
  .bn-modes button[aria-pressed="true"] {
    border-color: var(--faint);
    background: var(--pane-2);
    color: var(--text);
  }
  .bn-modes span {
    padding: 1px 7px;
    border-radius: 999px;
    background: #6eb6ff1f;
    font: 10.5px var(--mono);
    color: var(--cyan);
  }
  .bn-modes .bn-edit {
    background: #e07a5f26;
    color: var(--amber);
  }
  .bn-modes .bn-team {
    background: #a78bfa26;
    color: var(--indigo);
  }
  .bn-prompt,
  .bn-toml {
    border: 1px solid var(--line);
    border-radius: 10px;
    background: var(--bg);
  }
  /* Every mode sits in the same cell, so the card keeps the tallest one's height. */
  .bn-prompt {
    flex: 1;
    display: grid;
    padding: 16px 18px;
    font-size: 14px;
  }
  .bn-prompt > div {
    grid-area: 1 / 1;
    visibility: hidden;
  }
  .bn-prompt > .on {
    visibility: visible;
  }
  .bn-prompt p {
    margin: 6px 0 14px;
    font-size: 13.5px;
    color: var(--muted);
  }
  .bn-prompt blockquote {
    width: fit-content;
    margin-bottom: 10px;
    padding: 9px 13px;
    border: 1px solid #e07a5f4d;
    border-radius: 9px;
    background: #e07a5f14;
    font-size: 13.5px;
  }
  .bn-prompt span {
    font: 11.5px var(--mono);
    color: var(--amber);
  }

  .bn-shot {
    position: relative;
    padding-bottom: 0;
  }
  /* Crops the capture to the review pane: skip the sidebar and titlebar. */
  .bn-crop {
    position: relative;
    flex: 1;
    min-height: 300px;
    margin-right: -26px;
    overflow: hidden;
    border: 1px solid var(--line-bright);
    border-width: 1px 0 0 1px;
    border-radius: 10px 0 0;
    mask-image:
      linear-gradient(to right, #000 72%, transparent),
      linear-gradient(#000 62%, transparent);
    mask-composite: intersect;
  }
  .bn-crop > div {
    position: absolute;
    width: 175%;
    transform: translate(-19.4%, -4.7%);
  }

  .bn-agents {
    display: flex;
    flex-wrap: wrap;
    gap: 16px 40px;
  }
  .bn-agents li {
    display: flex;
    align-items: center;
    gap: 12px;
    font-size: 17px;
    font-weight: 500;
  }
  .bn-agents :global(svg) {
    width: 26px;
    height: 26px;
  }

  .bn-keys li {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 7px 0;
    border-top: 1px solid var(--line-soft);
    font-size: 13.5px;
    color: var(--muted);
  }
  .bn-keys span {
    display: flex;
    gap: 5px;
  }
  .bn kbd {
    min-width: 26px;
    padding: 2px 7px;
    border: 1px solid var(--line-bright);
    border-bottom-width: 2px;
    border-radius: 6px;
    background: var(--pane-2);
    font: 500 12px/1.5 system-ui, sans-serif;
    letter-spacing: 0.06em;
    text-align: center;
    color: var(--text);
  }

  .bn-toml {
    flex: 1 1 0;
    min-height: 300px;
    padding: 18px 20px;
    overflow: auto;
    font-size: 12.5px;
    line-height: 1.7;
  }
  .t-comment {
    color: var(--faint);
  }
  .t-table,
  .t-literal {
    color: var(--amber);
  }
  .t-key {
    color: #c4a0f5;
  }
  .t-string {
    color: #9fe3b5;
  }

  .bn-close {
    --text: #191716;
    --bg: #f8f5f2;
    --muted: #2b1d18;
    --faint: #382a25;
    --line-bright: #19171666;
    --pane-2: #19171618;
    --cyan: #191716;
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 36px;
    margin-top: 16px;
    padding: 52px;
    border-radius: 16px;
    background: var(--amber);
    color: var(--text);
  }
  .bn-close h2 {
    margin-bottom: 22px;
  }
  .bn-close ul {
    display: grid;
    gap: 12px;
    margin-bottom: 28px;
    font-size: 14px;
    color: var(--muted);
  }
  .bn-close li {
    position: relative;
    padding-left: 18px;
  }
  .bn-close li::before {
    content: "";
    position: absolute;
    left: 0;
    top: 0.72em;
    width: 7px;
    height: 2px;
    background: var(--text);
  }
  .bn-close .bn-actions {
    justify-content: flex-start;
  }
  .bn-close .bn-button:not(.bn-ghost):hover {
    background: #3a302c;
    border-color: #3a302c;
  }
  .bn-close > img {
    width: 182px;
    height: 182px;
    margin-right: 40px;
    border-radius: 42px;
    box-shadow: 0 20px 35px #43221644;
    rotate: 7deg;
  }
  .bn-footer {
    margin-top: 8px;
  }
  .bn-footer img {
    width: 80px;
    opacity: 0.7;
  }

  @media (min-width: 900px) {
    .bn-5 {
      grid-column: span 5;
    }
    .bn-6 {
      grid-column: span 6;
    }
    .bn-7 {
      grid-column: span 7;
    }
    .bn-wide {
      flex-direction: row;
      align-items: center;
      justify-content: space-between;
      gap: 40px;
    }
    .bn-wide ul {
      flex: none;
    }
  }
  @media (max-width: 600px) {
    .bn {
      padding-inline: 16px;
    }
    .bn-social {
      display: none;
    }
    .bn-hero {
      padding-top: 32px;
    }
    .bn-stage {
      margin-top: 40px;
    }
    .bn-features {
      padding-top: 64px;
    }
    .bn-card {
      padding: 20px;
    }
    .bn-crop {
      margin-right: -20px;
    }
    .bn-crop > div {
      width: 260%;
    }
    .bn-graph :global(.lf-foot) {
      grid-template-columns: 1fr;
    }
    .bn-commands li {
      grid-template-columns: 1fr;
    }
    .bn-close {
      padding: 30px 22px;
    }
    .bn-close > img {
      display: none;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .bn :global(*) {
      animation: none !important;
      transition: none !important;
    }
  }
</style>
