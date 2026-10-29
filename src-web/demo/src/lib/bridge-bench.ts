/**
 * One-shot Wails bridge RTT bench. Runs when CodeService.BridgeBenchEnabled()
 * is true (RECUR_BENCH_BRIDGE=1). Uses Call.ByName so we don't need regenerated IDs.
 */
import { Call } from '@wailsio/runtime'

const PING = 'recur/internal/app.CodeService.BridgePing'
const ECHO = 'recur/internal/app.CodeService.BridgeEcho'
const ENABLED = 'recur/internal/app.CodeService.BridgeBenchEnabled'
const DONE = 'recur/internal/app.CodeService.BridgeBenchDone'

function pct(sorted: number[], p: number): number {
  if (!sorted.length) return 0
  const i = Math.min(sorted.length - 1, Math.max(0, Math.ceil((p / 100) * sorted.length) - 1))
  return sorted[i]!
}

function summarize(samples: number[]) {
  const sorted = [...samples].sort((a, b) => a - b)
  const sum = sorted.reduce((a, b) => a + b, 0)
  return {
    n: sorted.length,
    min_ms: sorted[0] ?? 0,
    p50_ms: pct(sorted, 50),
    p90_ms: pct(sorted, 90),
    p99_ms: pct(sorted, 99),
    max_ms: sorted[sorted.length - 1] ?? 0,
    mean_ms: sorted.length ? sum / sorted.length : 0,
  }
}

async function timeCalls(n: number, fn: (i: number) => Promise<unknown>): Promise<number[]> {
  const samples: number[] = []
  for (let i = 0; i < n; i++) {
    const t0 = performance.now()
    await fn(i)
    samples.push(performance.now() - t0)
  }
  return samples
}

function makePayload(size: number): string {
  // ASCII-only so JSON length ≈ byte length (no UTF-16 surprises in the wire form).
  if (size <= 0) return ''
  const unit = 'abcdefghijklmnopqrstuvwxyz012345'
  return unit.repeat(Math.ceil(size / unit.length)).slice(0, size)
}

/** Run from App boot. No-op unless RECUR_BENCH_BRIDGE=1 on the Go side. */
export async function maybeRunBridgeBench(): Promise<void> {
  let enabled = false
  try {
    enabled = !!(await Call.ByName(ENABLED))
  } catch {
    return
  }
  if (!enabled) return

  try {
    // Warm the binding path (JIT / first-call setup).
    for (let i = 0; i < 30; i++) {
      await Call.ByName(PING, i)
    }

    const pingSamples = await timeCalls(500, (i) => Call.ByName(PING, i))

    const sizes = [0, 256, 4096, 65_536, 1_048_576]
    const echo: Record<string, ReturnType<typeof summarize>> = {}
    for (const size of sizes) {
      const payload = makePayload(size)
      await Call.ByName(ECHO, payload)
      const samples = await timeCalls(80, () => Call.ByName(ECHO, payload))
      echo[`${size}B`] = summarize(samples)
    }

    const stats = {
      when: new Date().toISOString(),
      platform: navigator.platform,
      userAgent: navigator.userAgent,
      ping_int64: summarize(pingSamples),
      echo_payload: echo,
      note: 'performance.now() around Call.ByName — JSON + WKWebView IPC + Go dispatch + return.',
    }

    console.info('[bridge-bench]', stats)
    await Call.ByName(DONE, JSON.stringify(stats))
  } catch (err) {
    console.error('[bridge-bench] failed', err)
    try {
      await Call.ByName(
        DONE,
        JSON.stringify({ error: String(err), when: new Date().toISOString() }),
      )
    } catch {
      /* ignore */
    }
  }
}
