<script lang="ts">
  import { flushSync } from "svelte";
  import manifest from "../public/screenshots/manifest.json";
  import ProductScreenshot from "./lib/components/ProductScreenshot.svelte";
  import ThemePreview from "./lib/components/ThemePreview.svelte";
  import LoopFlow from "./lib/components/LoopFlow.svelte";
  import ProviderIcon from "./lib/components/ProviderIcon.svelte";
  import ThemePickers from "./lib/components/ThemePickers.svelte";
  import { appThemeByKey, editorThemeByKey } from "./lib/themes";
  import { highlightToml } from "./lib/toml";
  const wordmark = "/images/recur-wordmark.png";
  const icon = "/images/recur-icon.png";
  const downloads =
    import.meta.env.VITE_DOWNLOADS_URL ||
    "https://github.com/celestiaorg/recur-releases/releases/latest/download";
  const github = "https://github.com/celestiaorg/recur";
  const linuxBuilds = [
    { arch: "amd64", label: "x64" },
    { arch: "arm64", label: "arm64" },
  ];
  const linuxFormats = ["deb", "rpm", "tar.gz"];
  const ua = navigator.userAgent;
  const onLinux = /Linux/.test(ua) && !/Android/.test(ua);
  const linuxFormat = /Fedora|Red Hat|CentOS|SUSE|Mageia/i.test(ua)
    ? "rpm"
    : "deb";
  // Chromium freezes the UA to x86_64, so ask Client Hints for the real arch.
  let linuxArch = $state(/aarch64|arm64|armv8/i.test(ua) ? "arm64" : "amd64");
  if (onLinux)
    navigator.userAgentData
      ?.getHighEntropyValues(["architecture"])
      .then(({ architecture }) => {
        if (architecture === "arm") linuxArch = "arm64";
      });
  const download = $derived(
    onLinux
      ? `${downloads}/Recur-linux-${linuxArch}.${linuxFormat}`
      : `${downloads}/Recur-macos-universal.dmg`,
  );
  const downloadLabel = onLinux
    ? `Download .${linuxFormat} for Linux`
    : "Download for Mac";
  const docs = "/docs/";

  // Follow the system until the visitor picks a theme. The choice shares the
  // docs' Vocs key so the splash and docs stay in step; ?theme= wins unsaved.
  const themeKey = "vocs-theme";
  const param = new URLSearchParams(location.search).get("theme");
  let stored: string | null = null;
  try {
    stored = localStorage.getItem(themeKey);
  } catch {}
  let picked = $state<"light" | "dark" | null>(
    param === "charcoal"
      ? "dark"
      : param === "paper"
        ? "light"
        : stored === "light" || stored === "dark"
          ? stored
          : null,
  );
  const systemQuery = matchMedia("(prefers-color-scheme: dark)");
  let systemDark = $state(systemQuery.matches);
  systemQuery.addEventListener("change", (e) => (systemDark = e.matches));
  const dark = $derived(picked ? picked === "dark" : systemDark);
  function pick(theme: "light" | "dark") {
    picked = theme;
    try {
      localStorage.setItem(themeKey, theme);
    } catch {}
  }
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
    | "issues"
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
      label: "Issues",
      screen: "issues",
      caption:
        "GitHub and Linear issues assigned to you or open in your projects. Start a workspace from any of them.",
    },
    {
      label: "Schedules",
      screen: "schedules",
      caption:
        "Prompts and loops on a cron, once, or at every launch. Each run gets a fresh workspace.",
    },
  ];
  let screen = $state<Screen>("workspace");
  const screenIndex = $derived(
    screens.findIndex((item) => item.screen === screen),
  );
  const selected = $derived(screens[screenIndex]);
  // The tour advances on its own until the visitor takes over. Each tab's
  // progress bar times its stop, so pausing the animation pauses the tour.
  const reducedMotion = matchMedia("(prefers-reduced-motion: reduce)").matches;
  let tourAuto = $state(!reducedMotion);
  let tourHover = $state(false);
  let tourFocus = $state(false);
  let tourHidden = $state(false);
  function observeTour(node: HTMLElement) {
    const obs = new IntersectionObserver(([e]) => {
      tourHidden = !e.isIntersecting;
    });
    obs.observe(node);
    return () => obs.disconnect();
  }
  const captureSrc = (id: string) =>
    `/screenshots/${manifest.captures.find((c) => c.id === id)!.file}`;
  // Warm every tour stop for the current theme so a switch never waits on
  // the network.
  $effect(() => {
    const theme = dark ? "charcoal" : "paper";
    for (const item of screens)
      new Image().src = captureSrc(`${item.screen}-${theme}`);
  });

  // Decode the next capture first, then crossfade where the browser can.
  async function showScreen(next: Screen) {
    if (next === screen) return;
    const image = new Image();
    image.src = captureSrc(`${next}-${dark ? "charcoal" : "paper"}`);
    await image.decode().catch(() => {});
    if (reducedMotion || !document.startViewTransition) {
      screen = next;
      return;
    }
    document.startViewTransition(() => flushSync(() => (screen = next)));
  }
  function stepScreen(delta: number) {
    const next = (screenIndex + delta + screens.length) % screens.length;
    void showScreen(screens[next].screen);
  }

  const pillars = [
    {
      title: "Done means reviewed.",
      body: "Every change gets a second set of eyes that can’t touch the code. Problems get fixed, or argued down, before they reach you.",
      code: "/review-and-fix",
    },
    {
      title: "Nothing is final.",
      body: "Every turn is a checkpoint. Rewind the chat and the code to any point, or branch off and try it another way.",
      code: "Reset to here",
    },
    {
      title: "Your process, your rules.",
      body: "How agents plan, build, and review lives in plain files. Make it strict, make it fast, make it yours.",
      code: "bundle.toml",
    },
    {
      title: "Works while you sleep.",
      body: "Put the chores on a schedule. Wake up to fixed findings, fresh PRs, and a short list of what needs you.",
      code: "@daily",
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
  const loopExample = highlightToml(`# ~/.recur/bundles/Team/bundle.toml
[[role]]
id = "team.roles.api-reviewer"
name = "API reviewer"
code = "AR"
mode = "review"
prompt = """
Review public API changes only. File a finding with
report_finding for each breaking change, missing
version bump, or undocumented error. Breaking is high.
"""

[[loop]]
id = "team.loops.api-change"
name = "API change"
conductor = "recur.roles.maestro"
entry = "plan"

[[loop.phase]]
id = "plan"
uses = "plan"
role = "recur.roles.planner"
prompt = "Plan from the OpenAPI spec. List every endpoint that changes."
gate = "plan_approval"
next = "build"

[[loop.phase]]
id = "build"
uses = "converge"
next = "check"

[loop.phase.roles]
implementer = "recur.roles.coder"
reviewer = "team.roles.api-reviewer"

[loop.phase.prompts]
implement = "Update the OpenAPI spec first, then the handlers."
fix = "/fix-findings"

[[loop.phase]]
id = "check"
uses = "verify"
role = "recur.roles.qa"
prompt = "Call each changed endpoint and attach the responses."
next = "pr"

[[loop.phase]]
id = "pr"
uses = "ensure_pr"`);
  // Lucide paths for modifier glyphs: fonts draw ⌘ ⇧ ⌥ at uneven sizes.
  const keyIcons: Record<string, string[]> = {
    "⌘": [
      "M15 6v12a3 3 0 1 0 3-3H6a3 3 0 1 0 3 3V6a3 3 0 1 0-3 3h12a3 3 0 1 0-3-3",
    ],
    "⇧": ["M9 18v-6H5l7-7 7 7h-4v6H9z"],
    "⌥": ["M3 3h6l6 18h6", "M14 3h7"],
    "⌃": ["m18 15-6-6-6 6"],
    "⌫": [
      "M10 5a2 2 0 0 0-1.344.519l-6.328 5.74a1 1 0 0 0 0 1.481l6.328 5.741A2 2 0 0 0 10 19h10a2 2 0 0 0 2-2V7a2 2 0 0 0-2-2z",
      "m12 9 6 6",
      "m18 9-6 6",
    ],
    "↑": ["m5 12 7-7 7 7", "M12 19V5"],
    "↓": ["M12 5v14", "m19 12-7 7-7-7"],
  };
  const keys: [string[], string][] = [
    [["⌘N"], "New workspace"],
    [["⌘K"], "Go to any workspace or PR"],
    [["⌘P"], "Open a file"],
    [["⌘⇧P"], "Run a bundle command"],
    [["⌘."], "Switch chat mode"],
    [["⌘L"], "Add the selection to chat"],
    [["⌘1", "⌘2", "⌘3"], "Focus chat, editor, terminal"],
    [["⌥⌘↑", "⌥⌘↓"], "Previous / next workspace"],
    [["J", "K"], "Next / previous file in review"],
    [["⌘R"], "Start a project run"],
    [["⌘⇧⌫"], "Stop the agent"],
    [["⌃`"], "Toggle the terminal"],
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

{#snippet external()}<svg
    class="b-icon"
    aria-hidden="true"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="2"
    stroke-linecap="round"
    stroke-linejoin="round"
    ><path d="M15 3h6v6" /><path d="M10 14 21 3" /><path
      d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"
    /></svg
  >{/snippet}

{#snippet docLink(path: string, label: string)}<a
    class="b-doc-link"
    href="{docs}{path}">{label} {@render external()}</a
  >{/snippet}

{#snippet keycap(chord: string)}<kbd role="img" aria-label={chord}
    >{#each [...chord] as ch, i (i)}{#if keyIcons[ch]}<svg
          class="b-key-icon"
          aria-hidden="true"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2.25"
          stroke-linecap="round"
          stroke-linejoin="round"
          >{#each keyIcons[ch] as d (d)}<path {d} />{/each}</svg
        >{:else}{ch}{/if}{/each}</kbd
  >{/snippet}

{#snippet githubIcon()}<svg
    class="b-github"
    aria-hidden="true"
    viewBox="0 0 16 16"
    fill="currentColor"
    ><path
      d="M8 0c4.42 0 8 3.58 8 8a8.013 8.013 0 0 1-5.45 7.59c-.4.08-.55-.17-.55-.38 0-.27.01-1.13.01-2.2 0-.75-.25-1.23-.54-1.48 1.78-.2 3.65-.88 3.65-3.95 0-.88-.31-1.59-.82-2.15.08-.2.36-1.02-.08-2.12 0 0-.67-.22-2.2.82-.64-.18-1.32-.27-2-.27-.68 0-1.36.09-2 .27-1.53-1.03-2.2-.82-2.2-.82-.44 1.1-.16 1.92-.08 2.12-.51.56-.82 1.28-.82 2.15 0 3.06 1.86 3.75 3.64 3.95-.23.2-.44.55-.51 1.07-.46.21-1.61.55-2.33-.66-.15-.24-.6-.83-1.23-.82-.67.01-.27.38.01.53.34.19.73.9.82 1.13.16.45.68 1.31 2.69.94 0 .67.01 1.3.01 1.49 0 .21-.15.45-.55.38A7.995 7.995 0 0 1 0 8c0-4.42 3.58-8 8-8Z"
    /></svg
  >{/snippet}

{#snippet downloadIcon()}<svg
    class="b-icon"
    aria-hidden="true"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="2"
    stroke-linecap="round"
    stroke-linejoin="round"
    ><path d="M12 17V3" /><path d="m6 11 6 6 6-6" /><path d="M19 21H5" /></svg
  >{/snippet}

{#snippet platforms(id: string)}
  <button
    type="button"
    class="b-more"
    popovertarget={id}
    style:anchor-name="--{id}">Other downloads ▾</button
  >
  <div {id} class="b-downloads" popover="auto" style:position-anchor="--{id}">
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
  style:--syn-a={editor.a}
  style:--syn-b={editor.b}
  style:--syn-c={editor.c}
  style:--syn-comment={editor.comment}
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
          href="#inbox">Inbox</a
        ><a
          class="b-nav-secondary"
          href={docs}>Docs {@render external()}</a
        ><a
          class="b-nav-secondary"
          href={github}
          aria-label="Recur on GitHub">{@render githubIcon()}</a
        >
        <div class="b-themes" role="group" aria-label="Page theme">
          <button
            type="button"
            aria-pressed={!dark}
            onclick={() => pick("light")}>☀ Paper</button
          ><button
            type="button"
            aria-pressed={dark}
            onclick={() => pick("dark")}>◐ Charcoal</button
          >
        </div>
        <a class="b-nav-download" href={download}>Download</a>
      </nav>
    </header>
    <main id="main">
      <section class="b-hero">
        <h1>Agents that don’t stop<br /><em>until it’s right.</em></h1>
        <div class="b-intro">
          <p>
            <strong>Agent work should arrive finished.</strong> Recur runs your
            agents through your processes, every task, every time. Stay in
            charge and ship more.
          </p>
          <div>
            <div class="b-actions">
              <a class="b-button" href={download}
                >{downloadLabel} {@render downloadIcon()}</a
              ><a class="b-button b-ghost" href={docs}>Docs {@render external()}</a>
            </div>
            {@render platforms("hero-downloads")}
          </div>
        </div>
      </section>

      <section
        class="b-product"
        id="product"
        aria-label="Recur screens"
        onpointerenter={() => (tourHover = true)}
        onpointerleave={() => (tourHover = false)}
        onpointerdown={() => (tourAuto = false)}
        onfocusin={() => (tourFocus = true)}
        {@attach observeTour}
        onfocusout={(event) => {
          if (!event.currentTarget.contains(event.relatedTarget as Node))
            tourFocus = false;
        }}
      >
        <div class="b-product-bar">
          <div class="b-screen-tabs" role="group" aria-label="Recur screens">
            {#each screens as item (item.screen)}<button
                type="button"
                aria-pressed={screen === item.screen}
                onclick={() => {
                  tourAuto = false;
                  void showScreen(item.screen);
                }}
                >{item.label}{#if tourAuto && screen === item.screen}<span
                    class="b-tour-progress"
                    class:paused={tourHover || tourFocus || tourHidden}
                    aria-hidden="true"
                    onanimationend={() => stepScreen(1)}
                  ></span>{/if}</button
              >{/each}
          </div>
        </div>
        <div class="b-window-scroll">
          <div class="b-window">
            <ProductScreenshot
              {screen}
              theme={dark ? "charcoal" : "paper"}
              eager
              caption={selected.caption}
              position="{screenIndex + 1} / {screens.length}"
              onprev={() => {
                tourAuto = false;
                stepScreen(-1);
              }}
              onnext={() => {
                tourAuto = false;
                stepScreen(1);
              }}
            />
          </div>
        </div>
        <div class="b-product-caption">
          <p aria-live={tourAuto ? "off" : "polite"}>{selected.caption}</p>
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
          >{@render docLink("reference/agents", "Provider setup")}
        </div>
      </div>

      <section class="b-flow" aria-labelledby="flow-title">
        <h2 id="flow-title" class="b-eyebrow">Why Recur</h2>
        <ol>
          {#each pillars as pillar, i (pillar.title)}
            <li>
              <span class="b-step">{i + 1}</span>
              <h3>{pillar.title}</h3>
              <p>{pillar.body}</p>
              <code>{pillar.code}</code>
            </li>
          {/each}
        </ol>
      </section>

      <section class="b-section" id="modes">
        <div class="b-heading">
          <h2>Pick a mode<br /><em>for each chat.</em></h2>
          <p>
            Sometimes you want an answer, sometimes a plan, sometimes a second
            opinion. Every chat picks its own agent and how far it’s allowed to
            go. Run as many as you like.
            <span class="b-doc-links"
              >{@render docLink("concepts/modes", "Chat modes docs")}</span
            >
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
          <h2>Set the bar.<br /><em>The loop keeps going.</em></h2>
          <p>
            Hand a task to a team instead of one agent. A planner scopes it, a
            coder builds it, a reviewer tears into it, and they go round until
            it’s right or they need you. A Maestro keeps it moving, and you can
            reshape any of it.
            <span class="b-doc-links"
              >{@render docLink("concepts/loop", "Loop mode docs")}{@render docLink(
                "bundles/loops",
                "Write your own loop",
              )}</span
            >
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
          <h2>Every PR,<br /><em>one place.</em></h2>
          <ul class="b-list">
            <li>
              Every open PR across your projects, not just the ones Recur
              opened, with the ones waiting on you surfaced.
            </li>
            <li>
              Review the way you already do. Agent findings sit right next to
              human comments.
            </li>
            <li>
              Red CI? Hand it to an agent. Behind base? One command. Any PR can
              become a workspace.
            </li>
          </ul>
          <span class="b-doc-links"
            >{@render docLink("concepts/pull-requests", "Pull request docs")}</span
          >
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

      <section class="b-control b-flip" id="inbox">
        <div class="b-control-copy">
          <h2>One inbox,<br /><em>every agent.</em></h2>
          <ul class="b-list">
            <li>
              Every project, every workspace, sorted and filtered. Find what
              needs you, what’s in progress, and what is ready to pick up.
            </li>
            <li>
              Questions and permission requests rise to the top, so no agent
              sits waiting on a tab you forgot to check.
            </li>
            <li>
              Unsure of where you were? Ask Recur about your work right from the
              inbox.
            </li>
          </ul>
          <span class="b-doc-links"
            >{@render docLink("concepts/inbox", "Inbox docs")}</span
          >
        </div>
        <div class="b-review-scroll">
          <div class="b-review">
            <ProductScreenshot
              screen="inbox"
              theme={dark ? "charcoal" : "paper"}
            />
          </div>
        </div>
      </section>

      <section class="b-split" id="schedules">
        <div>
          <h2>Put the chores<br /><em>on a schedule.</em></h2>
          <p>
            Nightly review passes, weekly security sweeps, a morning report. Any
            prompt or loop can run on its own clock, each time in a fresh
            workspace, so the results are ready when you sit down.
            <span class="b-doc-links"
              >{@render docLink("concepts/schedules", "Schedule docs")}</span
            >
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
              Recur is built from TextMate-inspired bundles, and so is almost
              everything in it: slash commands, loops, roles, issue lookup,
              languages, themes. Copy one, change it, and reload without
              restarting.
              <span class="b-doc-links"
                >{@render docLink("bundles", "Bundle docs")}{@render docLink(
                  "bundles/loops",
                  "Write your own loop",
                )}</span
              >
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
            class="b-code b-toml"
            tabindex="0"
            aria-label="Example custom loop in a user bundle">{#each loopExample as token, i (i)}<span
                class="t-{token.kind}">{token.text}</span
              >{/each}</pre>
        </div>
        <div class="b-customize">
          <div class="b-customize-head">
            <h3>Themes are bundles too.</h3>
            {@render docLink("reference/themes", "Theme docs")}
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
        </div>
      </section>

      <section class="b-section" id="keyboard">
        <div class="b-heading">
          <h2>Keep your hands<br /><em>on the keys.</em></h2>
          <p>
            Every workspace, file, chat, and command is a chord away, and the
            editor speaks Vim and Emacs. Press {@render keycap("⌘⇧/")} in the app
            for the full list. On Linux, {@render keycap("⌘")} is {@render keycap(
              "Ctrl",
            )}.
            <span class="b-doc-links"
              >{@render docLink("reference/shortcuts", "All keyboard shortcuts")}</span
            >
          </p>
        </div>
        <ul class="b-keys">
          {#each keys as [chords, label] (label)}<li>
              <span class="b-chords"
                >{#each chords as chord (chord)}{@render keycap(chord)}{/each}</span
              >{label}
            </li>{/each}
        </ul>
      </section>

      <section class="b-download" id="download">
        <div class="b-download-copy">
          <h2>Local by<br />design.</h2>
          <ul class="b-list">
            <li>Workspaces are git worktrees on your disk.</li>
            <li>Model calls go straight to the provider you picked.</li>
            <li>GitHub goes through your own <code>gh</code> login.</li>
          </ul>
          <div class="b-actions">
            <a class="b-button" href={download}
              >{downloadLabel} {@render downloadIcon()}</a
            >
          </div>
          {@render platforms("footer-downloads")}
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
      >
      <nav aria-label="Footer">
        <a href={docs}>Docs {@render external()}</a>
        <a href={github} aria-label="Recur on GitHub">{@render githubIcon()}</a>
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
    transition:
      background-color 0.15s,
      border-color 0.15s;
  }
  .b-nav-download:hover {
    background: var(--pane-2);
    border-color: var(--muted);
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
  .b-hero {
    padding: 44px 0 34px;
  }
  .b-hero h1 {
    font: 600 clamp(44px, 5.4vw, 78px)/1.02 var(--display);
    letter-spacing: -0.05em;
    margin: 0 0 26px;
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
    transition:
      background-color 0.15s,
      border-color 0.15s;
  }
  .b-button:hover {
    background: color-mix(in srgb, var(--text) 82%, var(--bg));
    border-color: color-mix(in srgb, var(--text) 82%, var(--bg));
  }
  .b-ghost {
    background: transparent;
    color: var(--text);
    border-color: var(--line-bright);
  }
  .b-ghost:hover {
    background: var(--pane-2);
    border-color: var(--muted);
  }
  .b-github {
    width: 1.15em;
    height: 1.15em;
    vertical-align: -0.2em;
  }
  .b-icon {
    width: 0.85em;
    height: 0.85em;
    margin-left: 0.15em;
    vertical-align: -0.08em;
  }
  .b-button .b-icon {
    margin-left: 0;
  }
  .b-more {
    margin-top: 12px;
    padding: 0;
    background: none;
    border: 0;
    font: 12px var(--mono);
    color: var(--muted);
    cursor: pointer;
  }
  .b-more:hover {
    color: var(--text);
  }
  .b-downloads {
    width: min(380px, calc(100vw - 32px));
    padding: 16px 18px;
    background: var(--pane);
    color: var(--text);
    border: 1px solid var(--line);
    border-radius: 6px;
    box-shadow: 0 12px 32px rgba(0, 0, 0, 0.18);
  }
  @supports (anchor-name: --a) {
    .b-downloads {
      inset: auto;
      top: anchor(bottom);
      left: anchor(left);
      margin: 8px 0;
      position-try-fallbacks: flip-block, flip-inline;
    }
  }
  .b-downloads dl > div {
    display: grid;
    grid-template-columns: 96px 1fr;
    align-items: center;
    padding: 8px 0;
    border-bottom: 1px solid var(--line-soft);
  }
  .b-downloads dt {
    font-size: 13px;
    font-weight: 600;
  }
  .b-downloads dd {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 8px;
  }
  .b-downloads dd a {
    padding: 4px 9px;
    border: 1px solid var(--line-bright);
    border-radius: 4px;
    font: 12px var(--mono);
  }
  .b-downloads dd a:hover {
    border-color: var(--amber);
    color: var(--amber);
  }
  .b-downloads span,
  .b-downloads p {
    font-size: 12px;
    color: var(--muted);
  }
  .b-downloads p {
    margin-top: 10px;
    line-height: 1.5;
  }
  .b-downloads p a {
    color: var(--text);
    text-decoration: underline;
  }

  .b-product {
    padding: 12px 14px 0;
    background:
      radial-gradient(
        120% 70% at 50% 0%,
        color-mix(in srgb, var(--amber) 7%, transparent),
        transparent 70%
      ),
      var(--pane);
    border: 1px solid var(--line);
    border-radius: 9px;
  }
  .b-product-bar {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0 1px 12px;
    gap: 20px;
  }
  .b-screen-tabs {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
  }
  .b-screen-tabs button {
    position: relative;
    overflow: hidden;
    font-size: 12px;
    padding: 7px 12px;
    border-radius: 4px;
    color: var(--muted);
  }
  .b-tour-progress {
    position: absolute;
    left: 0;
    bottom: 0;
    width: 100%;
    height: 2px;
    background: rgb(255 255 255 / 0.6);
    transform: scaleX(0);
    transform-origin: left;
    animation: b-tour 6s linear forwards;
  }
  .b-tour-progress.paused {
    animation-play-state: paused;
  }
  @keyframes b-tour {
    to {
      transform: scaleX(1);
    }
  }
  .b-screen-tabs button[aria-pressed="true"] {
    background: var(--amber);
    color: #fff;
    font-weight: 600;
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
  .b-providers > div > span :global(svg) {
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
    grid-template-columns: repeat(4, 1fr);
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
  .b-flip {
    grid-template-columns: 1.7fr 1fr;
  }
  .b-flip .b-control-copy {
    order: 2;
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
  .b-toml {
    max-height: 420px;
  }
  .b-toml .t-comment {
    color: var(--syn-comment);
  }
  .b-toml .t-table {
    color: var(--syn-a);
  }
  .b-toml .t-key {
    color: var(--syn-b);
  }
  .b-toml .t-string {
    color: var(--syn-c);
  }
  .b-toml .t-literal {
    color: var(--syn-a);
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
  .b-customize :global(.theme-pickers) {
    width: 100%;
    margin-bottom: 18px;
  }
  .b-custom-window {
    height: 400px;
    min-width: 1000px;
  }
  .b-doc-links {
    display: flex;
    flex-wrap: wrap;
    gap: 8px 20px;
    margin-top: 14px;
  }
  .b-doc-link {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    font-weight: 600;
    color: var(--amber);
  }
  .b-doc-link :global(.b-icon) {
    margin-left: 0;
  }
  #keyboard kbd {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 1px;
    vertical-align: middle;
    min-width: 2.2em;
    padding: 3px 8px;
    font:
      500 13px/1.4 system-ui,
      -apple-system,
      BlinkMacSystemFont,
      "Segoe UI",
      sans-serif;
    letter-spacing: 0.02em;
    text-align: center;
    color: var(--text);
    background: var(--pane);
    border: 1px solid var(--line-bright);
    border-bottom-width: 2px;
    border-radius: 5px;
  }
  .b-key-icon {
    width: 0.92em;
    height: 0.92em;
    flex: none;
  }
  #keyboard .b-heading kbd {
    font-size: 12px;
    padding: 1px 6px;
  }
  .b-keys {
    list-style: none;
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    column-gap: 40px;
    border-top: 1px solid var(--line);
  }
  .b-keys li {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 14px 0;
    border-bottom: 1px solid var(--line-soft);
    font-size: 14px;
    color: var(--muted);
  }
  .b-chords {
    display: flex;
    gap: 5px;
    flex: none;
    min-width: 140px;
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
  .b-download .b-button:hover {
    background: #3a302c;
    border-color: #3a302c;
  }
  .b-download .b-more {
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
    .b-keys {
      grid-template-columns: 1fr 1fr;
    }
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
    .b-flip .b-control-copy {
      order: 0;
    }
    .b-heading > p {
      justify-self: start;
    }
    .b-flow ol {
      grid-template-columns: repeat(2, 1fr);
      row-gap: 24px;
    }
    .b-flow li:nth-child(3) {
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
      margin: 0 0 18px;
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
    .b-keys {
      grid-template-columns: 1fr;
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
