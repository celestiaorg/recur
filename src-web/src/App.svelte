<script lang="ts">
  import AppDemo from "./lib/components/AppDemo.svelte";
  import ProductScreenshot from "./lib/components/ProductScreenshot.svelte";
  import LoopFlow from "./lib/components/LoopFlow.svelte";
  import ProviderIcon from "./lib/components/ProviderIcon.svelte";
  import {
    docs,
    downloadLabel,
    downloads,
    github,
    icon,
    linuxBuilds,
    linuxFormats,
    loopExample,
    telegram,
    visitor,
    wordmark,
    x,
    type Screen,
  } from "./lib/site.svelte";

  let playing = $state(true);

  const green = [
    ["Conflicts with its base", "Update with base, then /update if git is stuck", "3 tries"],
    ["A check fails", "/fix-checks", "3 tries"],
    ["New review comments", "/address-review", "3 runs"],
  ];
  const agents = [
    ["claude", "Claude Code"],
    ["cursor", "Cursor"],
    ["codex", "Codex"],
    ["antigravity", "Antigravity"],
    ["opencode", "OpenCode"],
  ];
  const local = [
    "Workspaces are git worktrees on your disk.",
    "Model calls go straight to the provider you picked.",
    "GitHub goes through your own gh login.",
    "API keys live in the macOS Keychain or your Linux keyring.",
    "Native Rust on macOS and Linux, not a browser in a window.",
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

{#snippet group(
  id: string,
  title: string,
  accent: string,
  copy: string,
  path?: string,
)}
  <header class="bn-group">
    <h2 {id}>{title} <em>{accent}</em></h2>
    <p>
      {copy}
      {#if path}<a href="{docs}{path}" aria-label="Docs: {title} {accent}"
          >Docs →</a
        >{/if}
    </p>
  </header>
{/snippet}

{#snippet shot(screen: Screen, span: 5 | 7)}
  <div class="bn-card bn-{span} bn-shot">
    <div class="bn-crop bn-crop-{screen}">
      <div><ProductScreenshot {screen} /></div>
    </div>
  </div>
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
        Recur runs the coding agents you already use through your process: plan,
        build, test, review, until the work clears your bar. Then it keeps the
        pull request green.
      </p>
      {@render cta("bn-hero-downloads")}

      <ul class="bn-logos" aria-label="Runs">
        {#each agents as [provider, name] (provider)}<li>
            <ProviderIcon {provider} />{name}
          </li>{/each}
        <li>+ OpenAI-compatible endpoints</li>
      </ul>

      <div class="bn-stage"><AppDemo /></div>
    </section>

    <section class="bn-features" aria-labelledby="bn-finished">
      {@render group(
        "bn-finished",
        "Agent work should",
        "arrive finished.",
        "Type /to-a-pr in any chat. A planner scopes the work, a coder builds it, and a separate reviewer sends it back until it clears your bar. Add your own checks as a gate, and review with a different model if you like.",
        "concepts/workflows",
      )}
      <div class="bn-card">
        <button
          type="button"
          class="bn-play"
          aria-pressed={playing}
          onclick={() => (playing = !playing)}
          >{playing ? "Ⅱ Pause" : "▶ Play"}</button
        >
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
      </div>
    </section>

    <section class="bn-features bn-split" aria-labelledby="bn-process">
      {@render group(
        "bn-process",
        "Your process,",
        "written down.",
        "A workflow is plain TOML: who takes part, in what order, and what has to pass. Use the shipped ones, copy and change them, or ask an agent to write yours. Recur lints it, and nothing installs until you approve.",
        "bundles/workflows",
      )}
      <!-- svelte-ignore a11y_no_noninteractive_tabindex (Keyboard users need to scroll this code sample.) -->
      <pre
        class="bn-toml"
        tabindex="0"
        aria-label="Example custom workflow in a user bundle">{#each loopExample as token, i (i)}<span
            class="t-{token.kind}">{token.text}</span
          >{/each}</pre>
    </section>

    <section class="bn-features" aria-labelledby="bn-going">
      {@render group(
        "bn-going",
        "It keeps going",
        "without you.",
        "While Recur is open, it looks after your pull requests and starts work when something happens.",
      )}
      <div class="bn-grid">
        <article class="bn-card bn-7">
          {@render head(
            "Your PRs stay green.",
            "Fixed in the PR’s own workspace while its chat is idle. Each fix is on until you turn it off.",
            "concepts/pull-requests#automatic-fixes",
          )}
          <div class="bn-scroll">
            <table>
              <thead>
                <tr>
                  {#each ["When", "Recur runs", "Stops after"] as h (h)}<th scope="col">{h}</th>{/each}
                </tr>
              </thead>
              <tbody>
                {#each green as [when, what, cap] (when)}<tr
                    ><th scope="row">{when}</th><td>{what}</td><td>{cap}</td
                    ></tr
                  >{/each}
              </tbody>
            </table>
          </div>
        </article>

        <article class="bn-card bn-5">
          {@render head(
            "Start on an event, not just a clock.",
            "Give a schedule a command. Every line it prints starts a run.",
            "concepts/schedules#watch",
          )}
          <!-- svelte-ignore a11y_no_noninteractive_tabindex (Keyboard users need to scroll this code sample.) -->
          <pre
            class="bn-toml bn-watch"
            tabindex="0"
            aria-label="Example watch schedule"><span class="t-comment"># command</span>
tail -F log/development.log \
  | grep --line-buffered ERROR

<span class="t-comment"># prompt</span>
<span class="t-string">Find the cause and fix it.</span>

<span class="t-comment"># a line arrives, a workspace starts</span>
<span class="t-literal">ERROR checkout: nil cart</span></pre>
        </article>
      </div>
    </section>

    <section class="bn-features" aria-labelledby="bn-charge">
      {@render group(
        "bn-charge",
        "You stay",
        "in charge.",
        "Answer, approve, fix, or merge from one Inbox. Or tell Ask Recur, and it starts the work, passes on your answer, and reports back.",
        "concepts/inbox",
      )}
      <div class="bn-grid">
        {@render shot("inbox", 5)}
        {@render shot("inbox-checkout", 7)}
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
  .bn-group a:hover,
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
  .bn-group a,
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

  .bn-logos {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 10px 28px;
    margin-top: 36px;
    list-style: none;
  }
  .bn-logos li {
    display: flex;
    align-items: center;
    gap: 9px;
    font-size: 14px;
    color: var(--muted);
  }
  .bn-logos :global(svg) {
    width: 18px;
    height: 18px;
  }
  .bn-stage {
    position: relative;
    margin-top: 40px;
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
  .bn-stage > :global(*) {
    position: relative;
  }
  /* Screenshots open full size on click; say so on hover, and always on touch. */
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
  .bn-shot:has(.bn-crop:hover)::after,
  .bn-shot:has(:global(.capture-trigger:focus-visible))::after {
    opacity: 1;
  }
  @media (hover: none) {
    .bn-shot::after {
      opacity: 1;
    }
  }
  .bn-crop,
  .bn-crop :global(.capture-trigger) {
    cursor: pointer;
  }
  .bn-features {
    padding-top: 120px;
  }
  .bn-group {
    margin-bottom: 36px;
    text-align: center;
  }
  .bn-group p {
    max-width: 640px;
    margin: 14px auto 0;
    font-size: 16px;
    color: var(--muted);
    text-wrap: pretty;
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
  .bn-head a,
  .bn-group a {
    white-space: nowrap;
    font-size: 13px;
  }
  .bn-play {
    z-index: 1;
    align-self: flex-start;
    margin-bottom: -50px;
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

  .bn-close ul {
    list-style: none;
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

  .bn-toml {
    border: 1px solid var(--line);
    border-radius: 10px;
    background: var(--bg);
  }

  .bn-shot {
    position: relative;
    padding: 0;
  }
  .bn-crop {
    position: relative;
    height: 520px;
    overflow: hidden;
    mask-image: linear-gradient(#000 82%, transparent);
  }
  .bn-crop > div {
    position: absolute;
  }
  /* Each shows one column of its capture, in percent of the capture. */
  .bn-crop-inbox > div {
    width: 424%;
    transform: translate(-19.6%, -32.4%);
  }
  .bn-crop-inbox-checkout > div {
    width: 279%;
    transform: translate(-64.3%, -4.8%);
  }

  .bn-toml {
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
    .bn-7 {
      grid-column: span 7;
    }
    .bn-split {
      display: grid;
      grid-template-columns: minmax(0, 5fr) minmax(0, 6fr);
      gap: 64px;
      align-items: center;
    }
    .bn-split .bn-group {
      margin: 0;
      text-align: left;
    }
    .bn-split p {
      margin-inline: 0;
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
      padding-top: 80px;
    }
    .bn-card:not(.bn-shot) {
      padding: 20px;
    }
    .bn-crop {
      height: 400px;
    }
    .bn-crop-inbox-checkout {
      height: 340px;
    }
    .bn-graph :global(.lf-foot) {
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
