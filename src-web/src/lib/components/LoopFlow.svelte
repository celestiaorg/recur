<script lang="ts">
  let { playing = true }: { playing?: boolean } = $props();
  // Animated branch graph of one Loop run (docs/loops-as-bundles.plan.md §5.1).
  // Time flows left → right. The trunk along the bottom is the parent loop's
  // branch; each child loop rises into a lane, codes ⇄ reviews on its own, and
  // merges back the moment it settles. Steps advance on a timer; the dots let you
  // scrub. Honors prefers-reduced-motion by showing the end state.
  const steps = [
    { t: "Plan", cap: "The Planner writes a structured plan: steps, the files each touches, and what depends on what." },
    { t: "Fan out", cap: "Every ready step gets its own child workspace on its own branch, running in parallel." },
    { t: "Code ⇄ review", cap: "In each child, the Coder implements and the Reviewer files findings until the policy's bar is met." },
    { t: "Merge + replan", cap: "S1 settles and merges straight back. Its handoff says rounding needs its own step, so Maestro replans: S5 joins the board." },
    { t: "Unblock", cap: "S2 lands. S4 and S5 were waiting on it, so both start at once while S3 is still running. No barrier." },
    { t: "Land", cap: "S4, S5, and finally S3 land. The parent never coded. It integrated." },
    { t: "Ship", cap: "The Reviewer checks the aggregate diff on the parent branch, then the loop opens the PR." },
  ] as const;

  let step = $state(0);
  let root = $state<HTMLElement | undefined>();

  $effect(() => {
    if (!root) return;
    if (matchMedia("(prefers-reduced-motion: reduce)").matches) {
      step = steps.length - 1;
      return;
    }
    if (!playing) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const tick = () => {
      const last = step === steps.length - 1;
      timer = setTimeout(() => {
        step = last ? 0 : step + 1;
        tick();
      }, last ? 6000 : 3800);
    };
    // Only play while on screen.
    const obs = new IntersectionObserver(([e]) => {
      clearTimeout(timer);
      if (e.isIntersecting) tick();
    }, { threshold: 0.3 });
    obs.observe(root);
    return () => {
      clearTimeout(timer);
      obs.disconnect();
    };
  });

  const on = (n: number) => step >= n;
  // Geometry: trunk at y=T; lanes above it at y=140 / 90 / 40. Labels for trunk
  // nodes sit below the trunk (y=T+22).
  const T = 200;
  const LBL = T + 22;
</script>

<div class="loop-flow" bind:this={root}>
  <svg class="lfg" viewBox="0 0 990 250" role="img" aria-label={steps[step].cap}>
    <!-- legend -->
    <g class="legend" aria-hidden="true">
      <circle class="node cyan" cx="770" cy="18" r="4" /><text class="lbl dim" x="780" y="22">code</text>
      <circle class="node indigo" cx="830" cy="18" r="4" /><text class="lbl dim" x="840" y="22">review</text>
      <circle class="node indigo" cx="900" cy="18" r="5" /><text class="lbl dim" x="910" y="22">merged</text>
    </g>

    <!-- parent trunk -->
    <line class="trunk" x1="40" y1={T} x2="950" y2={T} />
    <text class="lbl trunk-lbl" x="950" y={T - 14} text-anchor="end">parent · billing-fix</text>

    <!-- Plan -->
    <text class="lbl pop" class:on={on(0)} x="40" y={LBL} text-anchor="middle">Plan</text>

    <!-- fan out: S1 S2 S3 rise into lanes; each parks at its review dot -->
    {#each [
      { y: 140, s: "S1", name: "refund-net", mergeAt: 3 },
      { y: 90, s: "S2", name: "refund-cases", mergeAt: 4 },
      { y: 40, s: "S3", name: "balance-clamp", mergeAt: 5 },
    ] as c (c.s)}
      <path class="br draw" class:on={on(1)} pathLength="1" d="M40,{T} C78,{T} 68,{c.y} 110,{c.y} H150" />
      <text class="lbl cyan pop" class:on={on(1)} x="110" y={c.y - 9}>{c.s}</text>
      <text class="lbl dim pop" class:on={on(1)} x="128" y={c.y - 9}>{c.name}</text>
      <circle class="node cyan pop" class:on={on(1)} cx="150" cy={c.y} r="4" />
      <!-- code ⇄ review: the gap between the dots is only the dashed cycle until the
           child converges; the merge path then draws it solid -->
      <path class="cyc pop" class:on={on(2) && !on(c.mergeAt)} d="M150,{c.y} H190 A20,13 0 0 1 150,{c.y} Z" />
      <path class="br draw" class:on={on(c.mergeAt)} pathLength="1" d="M150,{c.y} H190" />
      <circle class="node indigo pop" class:on={on(2)} cx="190" cy={c.y} r="4" />
    {/each}
    <!-- painted after the branches so they leave from behind the dot -->
    <circle class="node amber pop" class:on={on(0)} cx="40" cy={T} r="6" />

    <!-- S1 merges first; its handoff triggers a replan -->
    <path class="mg draw" class:on={on(3)} pathLength="1" d="M190,140 H200 C230,140 220,{T} 250,{T}" />
    <circle class="node indigo pop" class:on={on(3)} cx="250" cy={T} r="5" />
    <text class="lbl indigo pop" class:on={on(3)} x="250" y={LBL} text-anchor="middle">S1 merged</text>
    <g class="pop late" class:on={on(3)}>
      <rect class="replan" x="286" y={T - 4} width="8" height="8" transform="rotate(45 290 {T})" />
      <text class="lbl amber" x="290" y={T - 12} text-anchor="middle">replan</text>
    </g>

    <!-- S2 merges -->
    <path class="mg draw" class:on={on(4)} pathLength="1" d="M190,90 H270 C300,90 300,{T} 330,{T}" />
    <text class="lbl indigo pop" class:on={on(4)} x="330" y={LBL} text-anchor="middle">S2 merged</text>

    <!-- Waiting steps: S4 from the plan, S5 added by the replan. Both depend on S2,
         so both start the moment it lands, while S3 is still running. -->
    {#each [
      { y: 140, s: "S4", name: "changelog", from: 1, why: "after S1, S2" },
      { y: 90, s: "S5", name: "rounding", from: 3, why: "via replan" },
    ] as w (w.s)}
      <circle class="ghost pop late" class:on={on(w.from) && !on(4)} cx="400" cy={w.y} r="5" />
      <text class="lbl cyan pop" class:on={on(w.from)} x="400" y={w.y - 9}>{w.s}</text>
      <text class="lbl dim pop" class:on={on(w.from)} x="418" y={w.y - 9}>{w.name} · {on(4) ? w.why : "waiting"}</text>
      <!-- late/later: S2 has to land before its dependents branch off -->
      <path class="br draw late" class:on={on(4)} pathLength="1" d="M330,{T} C368,{T} 358,{w.y} 400,{w.y} H440" />
      <circle class="node cyan pop late" class:on={on(4)} cx="440" cy={w.y} r="4" />
      <path class="cyc pop later" class:on={on(4) && !on(5)} d="M440,{w.y} H480 A20,13 0 0 1 440,{w.y} Z" />
      <path class="br draw" class:on={on(5)} pathLength="1" d="M440,{w.y} H480" />
      <circle class="node indigo pop later" class:on={on(4)} cx="480" cy={w.y} r="4" />
    {/each}
    <circle class="node indigo pop" class:on={on(4)} cx="330" cy={T} r="5" />

    <!-- Land: S4, then S5, then the slow S3 -->
    <path class="mg draw" class:on={on(5)} pathLength="1" d="M480,140 H500 C530,140 520,{T} 550,{T}" />
    <circle class="node indigo pop" class:on={on(5)} cx="550" cy={T} r="5" />
    <text class="lbl indigo pop" class:on={on(5)} x="550" y={LBL} text-anchor="middle">S4 merged</text>

    <path class="mg draw mid" class:on={on(5)} pathLength="1" d="M480,90 H560 C590,90 590,{T} 620,{T}" />
    <circle class="node indigo pop mid" class:on={on(5)} cx="620" cy={T} r="5" />
    <text class="lbl indigo pop mid" class:on={on(5)} x="620" y={LBL} text-anchor="middle">S5 merged</text>

    <path class="mg draw late" class:on={on(5)} pathLength="1" d="M190,40 H630 C665,40 655,{T} 690,{T}" />
    <circle class="node indigo pop late" class:on={on(5)} cx="690" cy={T} r="5" />
    <text class="lbl indigo pop late" class:on={on(5)} x="690" y={LBL} text-anchor="middle">S3 merged</text>

    <!-- parent: aggregate review → PR -->
    <g class="pop" class:on={on(6)}>
      <circle class="node indigo" cx="820" cy={T} r="6" />
      <text class="lbl" x="820" y={LBL} text-anchor="middle">Review the aggregate</text>
      <circle class="node emerald later" cx="950" cy={T} r="7" />
      <text class="lbl later" x="950" y={LBL} text-anchor="middle">Open PR</text>
    </g>
  </svg>

  <div class="lf-foot">
    <p class="lf-cap" aria-live="polite">
      <b>{steps[step].t}.</b>
      {steps[step].cap}
    </p>
    <div class="lf-dots" role="tablist" aria-label="Flow steps">
      {#each steps as s, i (s.t)}
        <button
          type="button"
          role="tab"
          class:on={i === step}
          class:past={i < step}
          aria-selected={i === step}
          aria-label={s.t}
          onclick={() => (step = i)}
        ></button>
      {/each}
    </div>
  </div>
</div>
