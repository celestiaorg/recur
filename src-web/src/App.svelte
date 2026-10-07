<script lang="ts">
  import ProductScreenshot from "./lib/components/ProductScreenshot.svelte";
  import ThemePreview from "./lib/components/ThemePreview.svelte";
  import LoopFlow from "./lib/components/LoopFlow.svelte";
  import ProviderIcon from "./lib/components/ProviderIcon.svelte";
  import ThemePickers from "./lib/components/ThemePickers.svelte";
  import { appThemeByKey, editorThemeByKey } from "./lib/themes";
  const wordmark = "/images/recur-wordmark.png";
  const icon = "/images/recur-icon.png";
  const download = "https://recur.build/download/recur.dmg";
  const docs = "https://docs.recur.build";
  const source = "https://github.com/mycodecrafting/recur";

  let dark = $state(
    new URLSearchParams(location.search).get("theme") === "charcoal",
  );
  const app = $derived(appThemeByKey(dark ? "default" : "paper"));
  const editor = $derived(editorThemeByKey(dark ? "recur" : "paper"));
  let playing = $state(true);
  let mode = $state("Agent");
  let customAppKey = $state<string | null>(null);
  let customEditorKey = $state<string | null>(null);
  const customApp = $derived(appThemeByKey(customAppKey ?? app.key));
  const customEditor = $derived(
    editorThemeByKey(customEditorKey ?? editor.key),
  );

  type Screen =
    | "workspace"
    | "changes"
    | "loop"
    | "pull-request"
    | "inbox"
    | "schedules";
  const screens: { label: string; screen: Screen; caption: string }[] = [
    {
      label: "Workspace",
      screen: "workspace",
      caption:
        "A workspace is a git worktree on its own branch. The chat, the file, and the changed-file list sit side by side.",
    },
    {
      label: "Diff",
      screen: "changes",
      caption:
        "Read the worktree diff against its base and leave inline comments before anything is pushed.",
    },
    {
      label: "Loop",
      screen: "loop",
      caption:
        "A To a PR loop fanned out into three child workspaces. Each one merges back as soon as it converges.",
    },
    {
      label: "Pull request",
      screen: "pull-request",
      caption:
        "Open PRs across all your projects, including ones opened outside Recur: conversation, diff, commits, and checks.",
    },
    {
      label: "Inbox",
      screen: "inbox",
      caption:
        "Every workspace in one list, grouped into needs you, working, and ready to pick up.",
    },
    {
      label: "Schedules",
      screen: "schedules",
      caption:
        "Prompts and loops on a cron, once, or at every launch. Each run gets a fresh workspace.",
    },
  ];
  let screen = $state<Screen>("workspace");
  const selected = $derived(screens.find((item) => item.screen === screen)!);

  const steps = [
    {
      title: "Task",
      body: "Describe it, or pull in a Linear issue from the palette.",
      code: "/linear",
    },
    {
      title: "Workspace",
      body: "A fresh git worktree off your base branch.",
      code: "main → recur/team-invites",
    },
    {
      title: "Agent",
      body: "Pick a provider, a model, and a mode for each chat.",
      code: "Claude Code · Agent",
    },
    {
      title: "Review",
      body: "Inline comments on the diff. Review chats file findings.",
      code: "/review",
    },
    {
      title: "Pull request",
      body: "Commit, push, and open it with an agent-written summary.",
      code: "/open-pr",
    },
  ];

  const modes = [
    {
      name: "Ask",
      access: "read",
      title: "Questions, answered from the repo.",
      body: "The agent explores the code and answers, or writes a report. Nothing in the worktree changes.",
      example: "Where does the session refresh happen, and what calls it?",
      outcome: "read-only · answers and reports",
    },
    {
      name: "Plan",
      access: "read",
      title: "Investigate before anyone edits.",
      body: "The agent traces the code and writes a plan: the files, the steps, the risks. Hand it to an Agent chat or a loop.",
      example: "Plan expiring team invitations for this app.",
      outcome: "read-only · a structured plan",
    },
    {
      name: "Agent",
      access: "edit",
      title: "Pair on the change.",
      body: "Reads and edits files in the workspace’s worktree. Steer it mid-turn or queue the next message, or let it spawn a child workspace for a subtask and merge the result back.",
      example: "Add a timeout to the session refresh request.",
      outcome: "read + edit · child workspaces",
    },
    {
      name: "Review",
      access: "read",
      title: "A second pass on the diff.",
      body: "Reviews the worktree diff and files findings with a severity and a location. Hand them back with /fix-findings.",
      example: "Review this change for bugs and unnecessary complexity.",
      outcome: "read-only · findings",
    },
    {
      name: "Loop",
      access: "team",
      title: "Hand the task to a team.",
      body: "Pick a loop and a policy in the composer. The Maestro runs specialist roles through the loop’s phases and reports back in this chat. Talk to it to steer the run.",
      example: "Add team invites with links that expire after 48 hours.",
      outcome: "Loop · To a PR · Balanced",
    },
  ];
  const activeMode = $derived(modes.find((item) => item.name === mode)!);

  const phases = [
    ["plan", "Planner writes a plan; the policy decides if you approve it."],
    ["converge", "Coder implements, Reviewer files findings, repeat."],
    ["verify", "Optional. QA checks the result in a real browser, with evidence."],
    ["ensure_pr", "Commit, push, open or update the pull request."],
    ["artifact", "Publish a document instead of a PR."],
  ];
  const policies = [
    ["Cautious", "any finding", "always", "2, escalate"],
    ["Balanced", "medium+", "if structured", "4, escalate"],
    ["Aggressive", "high+", "never", "6, proceed"],
  ];
  const loops = [
    "/to-a-pr",
    "/review-and-fix",
    "/review-to-pr",
    "/security-review",
    "/write-docs",
  ];
</script>

<svelte:head><meta name="theme-color" content={app.bg} /></svelte:head>

<div
  id="top"
  class="built"
  class:dark
  style:--bg={app.bg}
  style:--pane={app.pane}
  style:--pane-2={app.pane2}
  style:--line={app.line}
  style:--line-soft={app.lineSoft}
  style:--line-bright={app.lineBright}
  style:--text={app.text}
  style:--muted={app.muted}
  style:--faint={app.muted}
  style:--amber={app.accent}
  style:--amber-d={app.accentDim}
  style:--cyan={app.cyan}
  style:--emerald={app.emerald}
  style:--coral={app.coral}
  style:--indigo={app.indigo}
>
  <a class="b-skip" href="#main">Skip to content</a>
  <div class="b-wrap">
    <header class="b-header">
      <a href="#top" aria-label="Recur home"
        ><img
          class="b-wordmark"
          src={wordmark}
          alt="Recur"
          width="720"
          height="172"
        /></a
      >
      <nav aria-label="Main navigation">
        <a class="b-nav-secondary" href="#modes">Modes</a><a
          class="b-nav-secondary"
          href="#loops">Loops</a
        ><a class="b-nav-secondary" href="#review">Pull requests</a><a
          class="b-nav-secondary"
          href={docs}>Docs ↗</a
        ><a class="b-nav-secondary" href={source}>Source ↗</a>
        <div class="b-themes" role="group" aria-label="Page theme">
          <button
            type="button"
            aria-pressed={!dark}
            onclick={() => (dark = false)}>☀ Paper</button
          ><button
            type="button"
            aria-pressed={dark}
            onclick={() => (dark = true)}>◐ Charcoal</button
          >
        </div>
        <a class="b-nav-download" href={download}>Download</a>
      </nav>
    </header>
    <main id="main">
      <section class="b-hero">
        <div class="b-eyebrow">
          <span class="b-dot"></span>Mac app · Open source · Apache 2.0
        </div>
        <h1>Run coding agents in parallel.<br /><em>One worktree each.</em></h1>
        <div class="b-intro">
          <p>
            Recur is a Mac app that runs <strong>Claude Code</strong>,
            <strong>Cursor</strong>, and <strong>Codex</strong> in isolated git
            worktrees, each task on its own branch. Review the diff, work the
            findings, and open the pull request without leaving the window.
          </p>
          <div>
            <div class="b-actions">
              <a class="b-button" href={download}
                >Download for Mac <span aria-hidden="true">↓</span></a
              ><a class="b-button b-ghost" href={docs}>Docs ↗</a><a
                class="b-button b-ghost"
                href={source}>Source ↗</a
              >
            </div>
            <p class="b-meta">
              macOS 12 or later · Apple Silicon and Intel
            </p>
          </div>
        </div>
      </section>

      <section class="b-product" id="product" aria-label="Recur screens">
        <div class="b-product-bar">
          <div class="b-screen-tabs" role="group" aria-label="Recur screens">
            {#each screens as item (item.screen)}<button
                type="button"
                aria-pressed={screen === item.screen}
                onclick={() => (screen = item.screen)}>{item.label}</button
              >{/each}
          </div>
          <span class="b-eyebrow">Real captures · {app.name}</span>
        </div>
        <div class="b-window-scroll">
          <div class="b-window">
            <ProductScreenshot
              {screen}
              theme={dark ? "charcoal" : "paper"}
              eager
            />
          </div>
        </div>
        <div class="b-product-caption">
          <p aria-live="polite">{selected.caption}</p>
          <span>Click to view full size ↗</span>
        </div>
      </section>

      <div class="b-providers">
        <span class="b-eyebrow">Runs the agents you already use</span>
        <div>
          <span><ProviderIcon provider="claude" />Claude Code</span><span
            ><ProviderIcon provider="cursor" />Cursor</span
          ><span><ProviderIcon provider="codex" />Codex</span><span
            class="b-compatible"
            >+ OpenAI-compatible: OpenAI, OpenRouter, Groq, or a custom URL
            such as Ollama</span
          >
        </div>
      </div>

      <section class="b-flow" aria-labelledby="flow-title">
        <h2 id="flow-title" class="b-eyebrow">From task to pull request</h2>
        <ol>
          {#each steps as step, i (step.title)}
            <li>
              <span class="b-step">{i + 1}</span>
              <h3>{step.title}</h3>
              <p>{step.body}</p>
              <code>{step.code}</code>
            </li>
          {/each}
        </ol>
      </section>

      <section class="b-section" id="modes">
        <div class="b-heading">
          <h2>Pick a mode<br /><em>for each chat.</em></h2>
          <p>
            Every chat picks its own provider, model, and mode, and you can
            switch mid-chat. Run as many chats as you like, side by side in one
            workspace or across many. PR chats stay read-only: Ask, Plan, and
            Review.
          </p>
        </div>
        <div class="b-mode-tabs" role="group" aria-label="Chat modes">
          {#each modes as item (item.name)}<button
              type="button"
              aria-pressed={mode === item.name}
              onclick={() => (mode = item.name)}
              ><span>{item.access}</span>{item.name}</button
            >{/each}
        </div>
        <div class="b-mode-detail" aria-live="polite">
          <div>
            <h3>{activeMode.title}</h3>
            <p>{activeMode.body}</p>
          </div>
          <div class="b-mode-example">
            <span class="b-eyebrow">{activeMode.name} / Example prompt</span>
            <p>{activeMode.example}</p>
            <span>{activeMode.outcome}</span>
          </div>
        </div>
        <p class="b-simplify">
          <span>✓ Simplify</span>On by default in Agent and Plan chats: reuse
          before writing, stdlib first, the shortest diff that works. Toggle it
          per chat.
        </p>
      </section>

      <section class="b-section" id="loops">
        <div class="b-heading">
          <h2>Loops hand the task<br /><em>to a team of agents.</em></h2>
          <p>
            A loop is a graph of phases declared in a bundle. Each phase is run
            by a role with its own prompt, mode, and tools. The Maestro conducts:
            it reports status, edits the plan, extends rounds, and dispatches
            on-demand phases, but never does the phase work itself.
          </p>
        </div>
        <div class="b-graph-heading">
          <span class="b-eyebrow">To a PR / structured plan, fanned out</span
          ><button
            type="button"
            class="b-play"
            aria-pressed={playing}
            onclick={() => (playing = !playing)}
            >{playing ? "Ⅱ Pause animation" : "▶ Play animation"}</button
          >
        </div>
        <!-- svelte-ignore a11y_no_noninteractive_tabindex (Keyboard users need to scroll this wide preview.) -->
        <div
          class="b-graph-scroll"
          tabindex="0"
          role="region"
          aria-label="Animated loop graph. Scroll horizontally on small screens."
        >
          <div class="b-graph" class:paused={!playing}>
            <LoopFlow {playing} />
          </div>
        </div>
        <p class="b-swipe">Swipe to follow the branches →</p>
        <div class="b-specs">
          <article>
            <h3>Roles</h3>
            <p>
              Maestro, Planner, Coder, Reviewer, Security reviewer, QA, Writer,
              and Editor. Only the Coder edits. The Reviewer reads and files
              findings. QA has no file or shell access and drives a browser
              through Playwright.
            </p>
          </article>
          <article>
            <h3>Phases</h3>
            <dl>
              {#each phases as [kind, text] (kind)}<div>
                  <dt><code>{kind}</code></dt>
                  <dd>{text}</dd>
                </div>{/each}
            </dl>
          </article>
          <article>
            <h3>Policies</h3>
            <table>
              <thead>
                <tr
                  ><th scope="col">Policy</th><th scope="col">Blocks on</th><th
                    scope="col">Plan approval</th
                  ><th scope="col">Round cap</th></tr
                >
              </thead>
              <tbody>
                {#each policies as [name, blocks, approval, cap] (name)}<tr
                    ><th scope="row">{name}</th><td>{blocks}</td><td
                      >{approval}</td
                    ><td>{cap}</td></tr
                  >{/each}
              </tbody>
            </table>
          </article>
        </div>
        <p class="b-loops-list">
          <span class="b-eyebrow">Shipped loops</span>
          {#each loops as command (command)}<code>{command}</code>{/each}
        </p>
      </section>

      <section class="b-control" id="review">
        <div class="b-control-copy">
          <h2>Review and merge<br />pull requests.</h2>
          <ul class="b-list">
            <li>
              One Pull requests view across every project, including PRs opened
              outside Recur. It flags review requests, assignments, requested
              changes, and failing checks.
            </li>
            <li>
              Line comments batched into a review. Approve, request changes,
              reply, resolve, and react. Agent findings sit next to human
              comments.
            </li>
            <li>
              Mark ready, then squash, rebase, or merge. <code>/fix-checks</code>
              starts an agent on red CI; <code>/update</code> catches the branch
              up.
            </li>
            <li>
              Check out any PR into a new workspace. GitHub goes through the
              <code>gh</code> CLI, as the account you already use.
            </li>
          </ul>
        </div>
        <div class="b-review-scroll">
          <div class="b-review">
            <ProductScreenshot
              screen="pull-request"
              theme={dark ? "charcoal" : "paper"}
            />
          </div>
        </div>
      </section>

      <section class="b-split" id="schedules">
        <div>
          <h2>Run prompts on a schedule.</h2>
          <p>
            Save a prompt or a loop with a project, base branch, model, and
            mode. Each occurrence gets a fresh workspace. Recur previews the
            next three run times before you save, marks runs missed while it was
            closed instead of replaying them, and skips a run while the last one
            is still going.
          </p>
        </div>
        <!-- svelte-ignore a11y_no_noninteractive_tabindex (Keyboard users need to scroll this code sample.) -->
        <pre
          class="b-code"
          tabindex="0"
          aria-label="Example schedules"><span class="b-dim">NAME                   CADENCE           MODE</span>
Nightly review &amp; fix   0 2 * * 1-5       Loop · Review &amp; fix · Balanced
Weekly security pass   0 9 * * 1         Loop · Security review &amp; fix
Morning code review    @daily            Review
Refresh dev fixtures   @reboot           Agent
Release notes draft    once Oct 1 09:00  Agent</pre>
      </section>

      <section class="b-section" id="bundles">
        <div class="b-split">
          <div>
            <h2>Extend it<br /><em>with bundles.</em></h2>
            <p>
              Recur Bundles are TextMate-inspired directories with one
              <code>bundle.toml</code>. Slash commands, loops, roles, policies,
              editor macros, grammars, snippets, language servers, themes, and
              Linear search all ship this way. Copy a shipped bundle into
              <code>~/.recur/bundles</code> to override it by id, then pick
              Navigate → Reload Bundles to load it without restarting.
            </p>
            <!-- svelte-ignore a11y_no_noninteractive_tabindex (Keyboard users need to scroll this code sample.) -->
            <pre
              class="b-code"
              tabindex="0"
              aria-label="Recur command line"><span class="b-dim"># installed on your PATH at launch</span>
recur .
recur bundles lint ./bundles
recur bundles test ./bundles/JSON</pre>
          </div>
          <!-- svelte-ignore a11y_no_noninteractive_tabindex (Keyboard users need to scroll this code sample.) -->
          <pre
            class="b-code"
            tabindex="0"
            aria-label="Excerpt from bundles/Loop/bundle.toml"><span
              class="b-dim"># bundles/Loop/bundle.toml</span>
[[loop]]
id = "recur.loops.converge"
name = "Review &amp; fix"
conductor = "recur.roles.maestro"
entry = "converge"

[[loop.phase]]
id = "converge"
uses = "converge"
start = "review"
on_max_rounds = "escalate"

[loop.phase.roles]
implementer = "recur.roles.coder"
reviewer = "recur.roles.reviewer"</pre>
        </div>
        <div class="b-customize">
          <div class="b-customize-head">
            <h3>Themes are bundles too.</h3>
            <p>
              App and editor themes are picked separately. This is the real UI
              running in your browser.
            </p>
          </div>
          <ThemePickers
            bind:appKey={() => customApp.key, (key) => (customAppKey = key)}
            bind:editorKey={
              () => customEditor.key, (key) => (customEditorKey = key)
            }
          />
          <!-- svelte-ignore a11y_no_noninteractive_tabindex (Keyboard users need to scroll this wide preview.) -->
          <div
            class="b-window-scroll"
            tabindex="0"
            role="region"
            aria-label="Try app and editor themes. Scroll horizontally on small screens."
          >
            <div class="b-custom-window">
              <ThemePreview app={customApp} editor={customEditor} />
            </div>
          </div>
          <p class="b-swipe">Swipe to explore the editor →</p>
          <div class="b-shortcuts">
            <span><kbd>⌘K</kbd> Go to workspaces and PRs</span><span
              ><kbd>⌘⇧P</kbd> Run a bundle command</span
            ><span><kbd>⌘N</kbd> New workspace</span><span
              ><kbd>Vim / Emacs</kbd> Editor keymaps</span
            >
          </div>
        </div>
      </section>

      <section class="b-download" id="download">
        <div class="b-download-copy">
          <h2>Local and<br />open source.</h2>
          <ul class="b-list">
            <li>Workspaces are git worktrees on your disk.</li>
            <li>Model calls go straight to the provider you picked.</li>
            <li>GitHub goes through your own <code>gh</code> login.</li>
            <li>Licensed under Apache 2.0.</li>
          </ul>
          <div class="b-actions">
            <a class="b-button" href={download}
              >Download for Mac <span aria-hidden="true">↓</span></a
            ><a class="b-button b-ghost" href={source}>Source ↗</a>
          </div>
          <p class="b-meta">macOS 12 or later · Apple Silicon and Intel</p>
        </div>
        <img src={icon} alt="Recur app icon" width="204" height="204" />
      </section>
    </main>
    <footer class="b-footer">
      <a href="#top" aria-label="Recur home"
        ><img
          class="b-wordmark"
          src={wordmark}
          alt="Recur"
          width="720"
          height="172"
        /></a
      ><span class="b-eyebrow">Apache 2.0</span>
      <nav aria-label="Footer">
        <a href={docs}>Docs ↗</a><a href={source}>Source ↗</a>
      </nav>
    </footer>
  </div>
</div>

<style>
  .built {
    --display: "Bricolage Grotesque", "Avenir Next", sans-serif;
    --mono: "IBM Plex Mono", monospace;
    background: var(--bg);
    color: var(--text);
    font: 16px/1.6 var(--display);
    min-height: 100vh;
    color-scheme: light;
  }
  .built.dark {
    color-scheme: dark;
  }
  .b-wrap {
    width: min(1320px, calc(100% - 96px));
    margin: auto;
  }
  .built :global(a:focus-visible),
  .built :global(button:focus-visible),
  .built :global([tabindex]:focus-visible) {
    outline: 3px solid var(--cyan);
    outline-offset: 4px;
  }
  .built em {
    font-style: normal;
    color: var(--amber);
  }
  .built h2 {
    font: 600 clamp(36px, 3.9vw, 54px)/1.06 var(--display);
    letter-spacing: -0.045em;
  }
  .built h3 {
    font: 600 20px/1.25 var(--display);
    letter-spacing: -0.025em;
    margin: 0 0 10px;
  }
  .built code,
  .built kbd {
    font: 0.86em var(--mono);
  }
  .built p > code,
  .built li > code {
    padding: 1px 5px;
    border-radius: 3px;
    background: var(--amber-d);
    color: var(--text);
    white-space: nowrap;
  }
  .b-header {
    height: 88px;
    display: flex;
    justify-content: space-between;
    align-items: center;
    border-bottom: 1px solid var(--line);
  }
  .b-wordmark {
    width: 119px;
    height: auto;
    filter: brightness(0);
    object-fit: contain;
  }
  .dark .b-wordmark {
    filter: none;
  }
  .b-header nav {
    display: flex;
    align-items: center;
    gap: 24px;
    font-size: 12px;
  }
  .b-header nav > a:hover,
  .b-footer nav a:hover {
    color: var(--amber);
  }
  .b-nav-download {
    border: 1px solid var(--line-bright);
    padding: 9px 14px;
    border-radius: 4px;
  }
  .b-themes {
    display: flex;
    border: 1px solid var(--line);
    padding: 3px;
    border-radius: 6px;
    gap: 3px;
  }
  .b-themes button {
    padding: 6px 11px;
    border-radius: 3px;
    font-size: 11px;
    color: var(--muted);
  }
  .b-themes button[aria-pressed="true"] {
    background: var(--pane);
    color: var(--text);
    box-shadow: 0 1px 3px #0001;
  }
  .b-eyebrow {
    font: 10px/1.6 var(--mono);
    letter-spacing: 0.11em;
    text-transform: uppercase;
    color: var(--muted);
  }
  .b-dot {
    display: inline-block;
    width: 6px;
    height: 6px;
    background: var(--amber);
    border-radius: 50%;
    margin-right: 9px;
    vertical-align: 1px;
  }

  .b-hero {
    padding: 44px 0 34px;
  }
  .b-hero h1 {
    font: 600 clamp(44px, 5.4vw, 78px)/1.02 var(--display);
    letter-spacing: -0.05em;
    margin: 18px 0 26px;
  }
  .b-intro {
    display: grid;
    grid-template-columns: minmax(0, 620px) auto;
    justify-content: space-between;
    align-items: end;
    gap: 40px;
  }
  .b-intro > p {
    font-size: 17px;
    line-height: 1.7;
    color: var(--muted);
  }
  .b-intro strong {
    color: var(--text);
    font-weight: 600;
  }
  .b-actions {
    display: flex;
    gap: 10px;
    align-items: center;
    flex-wrap: wrap;
  }
  .b-button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 14px;
    background: var(--text);
    color: var(--bg);
    font-size: 13px;
    font-weight: 600;
    padding: 13px 18px;
    border: 1px solid var(--text);
    border-radius: 4px;
    transition: transform 0.2s;
  }
  .b-button:hover {
    transform: translateY(-2px);
  }
  .b-ghost {
    background: transparent;
    color: var(--text);
    border-color: var(--line-bright);
  }
  .b-meta {
    font: 10px/1.8 var(--mono);
    color: var(--muted);
    margin-top: 12px;
  }

  .b-product {
    padding: 12px 14px 0;
    background: color-mix(in srgb, var(--amber) 14%, var(--bg));
    border: 1px solid color-mix(in srgb, var(--amber) 26%, var(--line));
    border-radius: 9px;
  }
  .b-product-bar {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0 1px 12px;
    gap: 20px;
  }
  .b-product-bar > .b-eyebrow {
    font-size: 9px;
  }
  .b-screen-tabs {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
  }
  .b-screen-tabs button {
    font-size: 12px;
    padding: 7px 12px;
    border-radius: 4px;
    color: var(--muted);
  }
  .b-screen-tabs button[aria-pressed="true"] {
    background: var(--pane);
    color: var(--text);
    box-shadow: 0 1px 3px #0001;
  }
  .b-window-scroll {
    overflow: auto;
    border-radius: 6px;
    border: 1px solid var(--line-bright);
    box-shadow: 0 13px 36px #0002;
  }
  .b-window {
    width: 100%;
  }
  .b-product-caption {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 20px;
    padding: 13px 2px;
    font-size: 12px;
    color: var(--muted);
  }
  .b-product-caption span {
    font: 9px var(--mono);
    white-space: nowrap;
  }
  .b-product-caption p {
    min-height: 3.2em;
  }

  .b-providers {
    display: flex;
    justify-content: space-between;
    align-items: center;
    flex-wrap: wrap;
    gap: 18px 30px;
    padding: 28px 0;
    border-bottom: 1px solid var(--line);
  }
  .b-providers > div {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 16px 30px;
  }
  .b-providers > div > span {
    display: flex;
    align-items: center;
    gap: 9px;
    font-size: 16px;
    font-weight: 600;
  }
  .b-providers :global(svg) {
    height: 18px;
    width: 18px;
  }
  .b-providers > div > .b-compatible {
    font-size: 12px;
    font-weight: 400;
    color: var(--muted);
    gap: 5px;
  }

  .b-flow {
    padding: 44px 0 52px;
    border-bottom: 1px solid var(--line);
  }
  .built .b-flow > h2 {
    font: 10px/1.6 var(--mono);
    letter-spacing: 0.11em;
  }
  .b-flow ol {
    list-style: none;
    display: grid;
    grid-template-columns: repeat(5, 1fr);
    margin-top: 18px;
    border-top: 1px solid var(--line);
  }
  .b-flow li {
    padding: 20px 22px 0 0;
    position: relative;
  }
  .b-flow li + li {
    padding-left: 22px;
    border-left: 1px solid var(--line);
  }
  .b-step {
    font: 10px var(--mono);
    color: var(--amber);
  }
  .b-flow h3 {
    margin: 6px 0 6px;
    font-size: 18px;
  }
  .b-flow p {
    font-size: 13px;
    line-height: 1.6;
    color: var(--muted);
    min-height: 3.2em;
  }
  .b-flow li > code {
    display: inline-block;
    margin-top: 12px;
    padding: 4px 8px;
    border-radius: 4px;
    background: var(--amber-d);
    color: var(--amber);
    font-size: 11px;
  }

  .b-section {
    padding: 72px 0;
    border-bottom: 1px solid var(--line);
  }
  .b-heading {
    display: grid;
    grid-template-columns: 1.3fr 1fr;
    gap: 70px;
    align-items: end;
    margin-bottom: 34px;
  }
  .b-heading > p,
  .b-split p {
    font-size: 15px;
    line-height: 1.75;
    color: var(--muted);
    max-width: 480px;
  }
  .b-heading > p {
    justify-self: end;
  }

  .b-mode-tabs {
    display: grid;
    grid-template-columns: repeat(5, 1fr);
    border-block: 1px solid var(--line);
  }
  .b-mode-tabs button {
    text-align: left;
    padding: 16px 0;
    border-bottom: 3px solid transparent;
    font-size: 15px;
    color: var(--muted);
  }
  .b-mode-tabs button > span {
    font: 9px var(--mono);
    letter-spacing: 0.08em;
    text-transform: uppercase;
    margin-right: 12px;
  }
  .b-mode-tabs button[aria-pressed="true"] {
    border-bottom-color: var(--amber);
    color: var(--text);
  }
  .b-mode-tabs button[aria-pressed="true"] > span {
    color: var(--amber);
  }
  .b-mode-detail {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 55px;
    align-items: center;
    background: var(--pane);
    padding: 28px 32px;
    min-height: 200px;
    border-bottom: 1px solid var(--line);
  }
  .b-mode-detail h3 {
    font-size: 26px;
    line-height: 1.15;
  }
  .b-mode-detail > div > p {
    font-size: 14px;
    line-height: 1.75;
    color: var(--muted);
    max-width: 480px;
  }
  .b-mode-example {
    background: var(--bg);
    border: 1px solid var(--line);
    padding: 16px 20px;
    border-radius: 4px;
  }
  .b-mode-example > .b-eyebrow {
    font-size: 8px;
  }
  .b-mode-example > p {
    margin: 10px 0 !important;
    color: var(--text) !important;
  }
  .b-mode-example > span:last-child {
    font: 10px/1.7 var(--mono);
    color: var(--amber);
  }
  .b-simplify {
    padding-top: 18px;
    font-size: 13px;
    color: var(--muted);
  }
  .b-simplify > span {
    font: 10px var(--mono);
    color: var(--emerald);
    background: color-mix(in srgb, var(--emerald) 10%, transparent);
    border: 1px solid color-mix(in srgb, var(--emerald) 25%, var(--line));
    border-radius: 4px;
    padding: 5px 9px;
    margin-right: 12px;
    white-space: nowrap;
  }

  .b-graph-heading {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 14px 20px;
    background: var(--pane);
    border: 1px solid var(--line);
    border-radius: 7px 7px 0 0;
  }
  .b-graph-heading > .b-eyebrow {
    font-size: 9px;
  }
  .b-play {
    font: 10px var(--mono);
    padding: 5px;
    color: var(--muted);
    white-space: nowrap;
  }
  .b-graph-scroll {
    overflow: auto;
    border: 1px solid var(--line);
    border-top: 0;
    background: var(--pane);
    border-radius: 0 0 7px 7px;
  }
  .b-graph {
    min-width: 850px;
  }
  .b-graph.paused :global(*) {
    animation-play-state: paused !important;
  }
  .b-graph :global(.loop-flow) {
    border: 0;
    border-radius: 0;
    padding: 28px 23px 22px;
  }
  .b-graph :global(.lf-cap) {
    font: 12px/1.7 var(--display);
    min-height: 42px;
  }
  .b-graph :global(.lf-dots button) {
    height: 24px;
    border-radius: 3px;
    position: relative;
    background: transparent;
    width: 23px;
  }
  .b-graph :global(.lf-dots button:after) {
    content: "";
    height: 4px;
    border-radius: 2px;
    background: var(--line-bright);
    position: absolute;
    left: 0;
    right: 0;
    top: 10px;
  }
  .b-graph :global(.lf-dots button.on:after) {
    background: var(--amber);
  }
  .b-graph :global(.lf-dots button.past:after) {
    background: color-mix(in srgb, var(--amber) 45%, var(--line-bright));
  }
  .b-specs {
    display: grid;
    grid-template-columns: 1fr 1.2fr 1.2fr;
    gap: 40px;
    margin-top: 34px;
  }
  .b-specs p,
  .b-specs dd {
    font-size: 13px;
    line-height: 1.7;
    color: var(--muted);
  }
  .b-specs dl > div {
    display: grid;
    grid-template-columns: 84px 1fr;
    gap: 10px;
    padding: 5px 0;
    border-top: 1px solid var(--line-soft);
  }
  .b-specs dt code {
    font-size: 11px;
    color: var(--amber);
  }
  .b-specs table {
    width: 100%;
    border-collapse: collapse;
    font-size: 12px;
  }
  .b-specs th,
  .b-specs td {
    text-align: left;
    padding: 6px 8px 6px 0;
    border-top: 1px solid var(--line-soft);
    font-weight: 400;
    color: var(--muted);
  }
  .b-specs thead th {
    font: 9px var(--mono);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    border-top: 0;
  }
  .b-specs tbody th {
    color: var(--text);
    font-weight: 600;
  }
  .b-loops-list {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    margin-top: 30px;
  }
  .b-loops-list > .b-eyebrow {
    margin-right: 8px;
  }
  .b-loops-list code {
    font-size: 11px;
    padding: 4px 8px;
    border-radius: 4px;
    background: var(--amber-d);
    color: var(--amber);
  }

  .b-control {
    display: grid;
    grid-template-columns: 1fr 1.7fr;
    gap: 52px;
    align-items: center;
    padding: 72px 0;
    border-bottom: 1px solid var(--line);
  }
  .b-control h2 {
    margin-bottom: 24px;
  }
  .b-list {
    list-style: none;
    display: grid;
    gap: 12px;
  }
  .b-list li {
    position: relative;
    padding-left: 18px;
    font-size: 14px;
    line-height: 1.65;
    color: var(--muted);
  }
  .b-list li:before {
    content: "";
    position: absolute;
    left: 0;
    top: 0.68em;
    width: 7px;
    height: 2px;
    background: var(--amber);
  }
  .b-review-scroll {
    overflow: auto;
    border: 1px solid var(--line-bright);
    border-radius: 7px;
    box-shadow: 0 10px 30px #0001;
  }
  .b-review {
    width: 100%;
  }

  .b-split {
    display: grid;
    grid-template-columns: 1fr 1.25fr;
    gap: 56px;
    align-items: start;
  }
  #schedules {
    padding: 72px 0;
    border-bottom: 1px solid var(--line);
    align-items: center;
  }
  .b-split h2 {
    margin-bottom: 20px;
  }
  .b-code {
    font: 12px/1.75 var(--mono);
    background: var(--pane);
    border: 1px solid var(--line);
    border-radius: 7px;
    padding: 20px 22px;
    overflow: auto;
    min-width: 0;
    color: var(--text);
  }
  .b-split > div > .b-code {
    margin-top: 24px;
  }
  .b-dim {
    color: var(--muted);
  }

  .b-customize {
    margin-top: 56px;
  }
  .b-customize-head {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    flex-wrap: wrap;
    gap: 8px 30px;
    margin-bottom: 20px;
  }
  .b-customize-head p {
    font-size: 13px;
    color: var(--muted);
  }
  .b-customize :global(.theme-pickers) {
    width: 100%;
    margin-bottom: 18px;
  }
  .b-custom-window {
    height: 400px;
    min-width: 1000px;
  }
  .b-shortcuts {
    display: flex;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 16px 20px;
    margin-top: 20px;
    color: var(--muted);
    font-size: 11px;
  }
  .b-shortcuts > span {
    display: flex;
    gap: 10px;
    align-items: center;
  }
  .b-shortcuts kbd {
    font-size: 10px;
    background: var(--pane);
    border: 1px solid var(--line-bright);
    box-shadow: 0 2px var(--line);
    padding: 4px 8px;
    border-radius: 4px;
    color: var(--text);
  }

  .b-download {
    margin-top: 72px;
    padding: 52px;
    background: var(--amber);
    color: #191716;
    border-radius: 7px;
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 35px;
  }
  .b-download h2 {
    margin-bottom: 22px;
  }
  .b-download .b-list {
    margin-bottom: 28px;
  }
  .b-download .b-list li {
    color: #2b1d18;
  }
  .b-download .b-list li:before {
    background: #191716;
  }
  .b-download .b-list code {
    background: #19171618;
    color: #191716;
  }
  .b-download .b-button {
    background: #191716;
    color: #f8f5f2;
    border-color: #191716;
  }
  .b-download .b-ghost {
    background: transparent;
    color: #191716;
  }
  .b-download .b-meta {
    color: #382a25;
  }
  .b-download > img {
    border-radius: 42px;
    box-shadow: 0 20px 35px #43221644;
    transform: rotate(7deg);
    margin-right: 40px;
    width: 182px;
    height: 182px;
  }
  .b-footer {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 25px;
    padding: 36px 0;
  }
  .b-footer .b-wordmark {
    width: 93px;
  }
  .b-footer > .b-eyebrow {
    font-size: 9px;
  }
  .b-footer nav {
    display: flex;
    gap: 22px;
    font-size: 12px;
    color: var(--muted);
  }
  .b-swipe {
    display: none;
  }
  .b-skip {
    position: absolute;
    left: 16px;
    top: -80px;
    background: var(--text);
    color: var(--bg);
    padding: 10px;
    z-index: 50;
  }
  .b-skip:focus {
    top: 10px;
  }

  @media (max-width: 1050px) {
    .b-wrap {
      width: calc(100% - 48px);
    }
    .b-header nav {
      gap: 15px;
    }
    .b-nav-secondary:nth-child(-n + 3) {
      display: none;
    }
    .b-intro,
    .b-heading,
    .b-control,
    .b-split {
      grid-template-columns: 1fr;
      gap: 26px;
    }
    .b-heading > p {
      justify-self: start;
    }
    .b-flow ol {
      grid-template-columns: repeat(3, 1fr);
      row-gap: 24px;
    }
    .b-flow li:nth-child(4) {
      padding-left: 0;
      border-left: 0;
    }
    .b-specs {
      grid-template-columns: 1fr 1fr;
    }
    .b-specs article:first-child {
      grid-column: 1 / -1;
    }
    .b-download {
      padding: 40px;
    }
    .b-download > img {
      margin-right: 15px;
      width: 150px;
      height: 150px;
    }
  }
  @media (max-width: 700px) {
    .b-wrap {
      width: calc(100% - 36px);
    }
    .b-header {
      height: 72px;
    }
    .b-wordmark {
      width: 97px;
    }
    .b-header nav {
      gap: 10px;
    }
    .b-nav-secondary,
    .b-nav-download {
      display: none;
    }
    .b-themes button {
      font-size: 10px;
      padding: 6px 8px;
    }
    .b-hero {
      padding: 30px 0 26px;
    }
    .b-hero h1 {
      font-size: clamp(36px, 10.4vw, 56px);
      margin: 16px 0 18px;
    }
    .b-hero .b-eyebrow {
      font-size: 9px;
    }
    .b-intro > p {
      font-size: 15px;
    }
    .b-actions .b-button:first-child {
      flex-basis: 100%;
    }
    .b-actions .b-ghost {
      flex: 1;
    }
    .b-product {
      padding: 8px 8px 0;
    }
    .b-product-bar {
      padding-bottom: 8px;
    }
    .b-product-bar > .b-eyebrow {
      display: none;
    }
    .b-screen-tabs {
      gap: 2px;
    }
    .b-screen-tabs button {
      padding: 7px 7px;
      font-size: 11px;
    }
    .b-product-caption {
      padding: 10px 2px;
      font-size: 11px;
    }
    .b-product-caption span {
      display: none;
    }
    .b-product-caption p {
      min-height: 4.8em;
    }
    .b-providers {
      padding: 22px 0;
    }
    .b-providers > div > span {
      font-size: 14px;
    }
    .b-flow {
      padding: 34px 0 38px;
    }
    .b-flow ol {
      grid-template-columns: 1fr;
      row-gap: 0;
    }
    .b-flow li,
    .b-flow li + li {
      display: grid;
      grid-template-columns: 22px 1fr;
      column-gap: 8px;
      padding: 14px 0;
      border-left: 0;
      border-bottom: 1px solid var(--line-soft);
    }
    .b-flow li > :not(.b-step) {
      grid-column: 2;
    }
    .b-step {
      grid-row: 1;
      padding-top: 3px;
    }
    .b-flow h3 {
      margin: 0 0 4px;
    }
    .b-flow p {
      min-height: 0;
    }
    .b-flow li > code {
      justify-self: start;
      margin-top: 8px;
    }
    .b-section,
    .b-control,
    #schedules {
      padding: 46px 0;
    }
    .built h2 {
      font-size: 36px;
    }
    .b-heading {
      margin-bottom: 24px;
    }
    .b-heading > p,
    .b-split p {
      font-size: 14px;
    }
    .b-mode-tabs button {
      font-size: 13px;
      padding: 12px 0;
    }
    .b-mode-tabs button > span {
      display: block;
      margin: 0 0 4px;
    }
    .b-mode-detail {
      grid-template-columns: 1fr;
      padding: 22px 18px;
      gap: 20px;
      min-height: 360px;
      align-items: start;
    }
    .b-mode-detail h3 {
      font-size: 23px;
    }
    .b-mode-detail > div > p {
      font-size: 13px;
    }
    .b-simplify {
      font-size: 12px;
      line-height: 1.8;
    }
    .b-graph-heading {
      padding: 10px;
    }
    .b-graph-heading > .b-eyebrow {
      font-size: 8px;
    }
    .b-play {
      font-size: 9px;
    }
    .b-specs {
      grid-template-columns: 1fr;
      gap: 26px;
    }
    .b-code {
      font-size: 11px;
      padding: 16px;
    }
    .b-custom-window {
      height: 380px;
    }
    .b-shortcuts {
      display: grid;
      grid-template-columns: 1fr 1fr;
      gap: 14px 12px;
    }
    .b-shortcuts > span {
      font-size: 10px;
      flex-wrap: wrap;
      gap: 6px;
    }
    .b-swipe {
      display: block;
      font: 9px var(--mono);
      color: var(--muted);
      margin-top: 10px;
    }
    .b-download {
      margin-top: 46px;
      padding: 30px 22px;
    }
    .b-download > img {
      display: none;
    }
    .b-footer {
      flex-wrap: wrap;
      gap: 18px;
      padding: 26px 0;
    }
    .b-footer > .b-eyebrow {
      order: 3;
      flex-basis: 100%;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .built :global(*),
    .built :global(*:before),
    .built :global(*:after) {
      animation: none !important;
      transition: none !important;
    }
  }
</style>
