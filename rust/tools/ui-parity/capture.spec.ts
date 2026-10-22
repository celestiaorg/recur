// Captures one reference PNG per scene in scenes.json into out/reference/.
//
// Fonts: index.html links Google Fonts exactly like frontend/index.html. That request is
// answered with @font-face rules for the very files recur-ui bundles, so both renderers
// rasterize identical font binaries and the capture never touches the network.
import { test, type Page } from '@playwright/test'
import { mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { installBridge, loadSeed } from '../../../frontend/e2e/helpers/mock-bridge'

const here = dirname(fileURLToPath(import.meta.url))
const fontsDir = join(here, '..', '..', 'crates', 'ui', 'assets', 'fonts')
const out = join(here, 'out', 'reference')
const spec = JSON.parse(readFileSync(join(here, 'scenes.json'), 'utf8')) as {
  scenes: Scene[]
  terminal?: Scene[]
  terminalInputs?: Record<string, string>
  shell?: Scene[]
  content?: Scene[]
  chat?: Scene[]
  review?: Scene[]
  observatory?: Scene[]
  workspace?: Scene[]
  prs?: Scene[]
}
// Terminal scenes replay `terminalInputs[input]` as the session's attach bytes; shell scenes
// mount App.svelte over an edited seed (see `patchSeed`).
const scenes = [
  ...spec.scenes, ...(spec.terminal ?? []), ...(spec.shell ?? []), ...(spec.content ?? []), ...(spec.chat ?? []),
  ...(spec.review ?? []), ...(spec.observatory ?? []), ...(spec.workspace ?? []), ...(spec.prs ?? []),
]
const only = process.env.UI_PARITY_SCENES?.split(',').filter(Boolean)
const seed = loadSeed()
const review = JSON.parse(readFileSync(join(here, 'content', 'review.json'), 'utf8'))

/** The large review scene's files, as `recur_view_review::gallery::large_files` builds them. */
function largeFiles(count: number) {
  return Array.from({ length: count }, (_, i) => {
    const n = i % 11 === 0 ? 400 : 2 + (i * 7) % 15
    const lines: { old: string; new: string; type: string; codeHtml: string }[] = [
      { old: '', new: '', type: 'hunk', codeHtml: `@@ -1,${n} +1,${n} @@` },
    ]
    for (let k = 1; k <= n; k++) {
      const type = k % 3 === 0 ? 'add' : k % 5 === 0 ? 'del' : 'ctx'
      const width = (k + i) % 4 === 0 ? 40 : 6
      lines.push({ old: type === 'add' ? '' : String(k), new: type === 'del' ? '' : String(k), type, codeHtml: `l${k} f${i} ${'word '.repeat(width)}` })
    }
    return { path: `src/pkg${i % 9}/file${i}.ts`, status: 'm', diff: { add: Math.floor(n / 3), del: Math.floor(n / 5) }, lines }
  })
}

/** Review scenes answer the review calls with `content/review.json`. */
function reviewResolvers(name: string) {
  const large = name.startsWith('review/large')
  return {
    DiffVsBase: () => ({ files: large ? largeFiles(review.large.files) : review.files }),
    DiffWorking: () => ({ files: review.files.filter((f: { path: string }) => review.dirty.includes(f.path)) }),
    ListCommits: () => review.commits,
    CommitDiff: () => [review.files[2]],
  }
}

const observatory = JSON.parse(readFileSync(join(here, 'content', 'observatory.json'), 'utf8'))

/** Observatory scenes answer the tree, file, git, artifact, terminal and editor-session calls
 *  with `content/observatory.json`, as `recur_view_observatory::gallery::bridge` does. */
function observatoryResolvers(name: string) {
  const variant = name.split('/').slice(1).join('/').replace(/-paper$/, '')
  const scene = observatory.scenes[variant] ?? {}
  const text = (path: string) => observatory.files[path]?.content as string | undefined
  return {
    ListTree: (a: any[]) => observatory.tree[(a[2] as string) || ''] ?? [],
    ReadFile: (a: any[]) => {
      const path = String(a[2] ?? '')
      const f = observatory.files[path]
      if (!f) throw new Error(`no such file: ${path}`)
      const size = f.asset ? readFileSync(join(here, 'content', f.asset)).length : Buffer.byteLength(f.content)
      return { path, language: f.language ?? '', content: f.content ?? '', size, writable: !!a[1], mime: f.mime ?? '' }
    },
    FileAtBase: (a: any[]) => observatory.base[String(a[2] ?? '')] ?? text(String(a[2] ?? '')) ?? '',
    Outline: (a: any[]) => observatory.outline[String(a[2] ?? '')] ?? [],
    GitStatus: () => observatory.gitStatus,
    WorkingStatus: () => observatory.working,
    StagedFiles: () => observatory.staged,
    ListChangedFiles: () => Object.keys(observatory.gitStatus),
    ListArtifacts: () => observatory.artifacts,
    GetEditorSession: () => structuredClone(scene.session ?? null),
    ListTerminals: () => (scene.terminal ? [observatory.terminal.session] : []),
    RestoreWorkspaceTerminals: () => (scene.terminal ? [observatory.terminal.session] : []),
    AttachTerminal: () => Buffer.from(observatory.terminal.input, 'utf8').toString('base64'),
  }
}

const workspace = JSON.parse(readFileSync(join(here, 'content', 'workspace.json'), 'utf8'))
const chat = JSON.parse(readFileSync(join(here, 'content', 'chat.json'), 'utf8'))

/** Workspace scenes answer the Observatory's calls for the scene's `session` and the chat's
 *  from `content/workspace.json`, as `recur_view_workspace::gallery::resolvers` does. */
function workspaceResolvers(name: string) {
  const variant = name.split('/').slice(1).join('/').replace(/-paper$/, '')
  const scene = workspace.scenes[variant] ?? {}
  const items = chat[scene.items]?.items ?? []
  return {
    ...observatoryResolvers(`observatory/${scene.session}`),
    ListLoopAgents: () => structuredClone(scene.agents ?? []),
    WorkspaceChatItems: () => structuredClone(items),
    WorkspaceSessions: () => structuredClone(scene.sessions ?? []),
    WorkspaceFacts: () => structuredClone(scene.facts ?? []),
    ListFindings: () => structuredClone(scene.findings ?? []),
    ...(scene.chats ? { ListWorkspaceChats: () => structuredClone(scene.chats) } : {}),
  }
}

const prs = JSON.parse(readFileSync(join(here, 'content', 'prs.json'), 'utf8'))

/** A PR scene's fixture in `content/prs.json`: its seed edits and canned replies. */
function prsFixture(scene: Scene): { seed?: SeedPatch; resolvers?: Record<string, unknown> } {
  return prs.fixtures[scene.fixture ?? ''] ?? {}
}

/** PR scenes answer the calls their fixture lists in `resolvers` with its canned value, as
 *  `recur_view_prs::gallery` does. */
function prsResolvers(scene: Scene) {
  const out: Record<string, () => unknown> = {}
  for (const [k, v] of Object.entries(prsFixture(scene).resolvers ?? {})) out[k] = () => structuredClone(v)
  return out
}

/** `/workspace-file?…&path=…[&at=base]`: the fixture's raster and PDF bytes. */
function workspaceFile(url: string) {
  const q = new URL(url).searchParams
  const path = q.get('path') ?? ''
  const asset = q.get('at') === 'base' ? observatory.baseAssets[path] : observatory.files[path]?.asset
  return asset ? readFileSync(join(here, 'content', asset)) : undefined
}

type Step =
  | { move: [number, number] }
  | { down: [number, number] }
  | { up: [number, number] }
  | { click: [number, number] }
  | { key: string }
  | { type: string }
  | { wait: number }
type Scene = {
  name: string
  width: number
  height: number
  theme: string
  steps?: Step[]
  animationTime?: number
  input?: string
  seed?: SeedPatch
  storage?: Record<string, string>
  treeScroll?: number
  route?: string
  pr?: string
  tab?: string
  dock?: string
  seg?: string
  fixture?: string
}
type Row = Record<string, unknown> & { id: string }
type SeedPatch = {
  projects?: Row[]
  loops?: Row[]
  prs?: Row[]
  projectGroups?: unknown[]
  inbox?: unknown[]
  settings?: Record<string, unknown>
}

/** A shell scene's seed edits, as `recur_app::gallery::patch_seed` applies them: `projects`,
 *  `loops` and `prs` upsert rows by id (mirrored into `loopById` / `prById`), `projectGroups`
 *  and `inbox` replace, `settings` merges. */
function patchSeed(seed: Record<string, any>, patch: SeedPatch | undefined) {
  if (!patch) return
  const upsert = (list: string, byId: string | null, rows: Row[] | undefined) => {
    for (const row of rows ?? []) {
      const arr: Row[] = (seed[list] ??= [])
      const hit = arr.find((x) => x.id === row.id)
      if (hit) Object.assign(hit, structuredClone(row))
      else arr.push(structuredClone(row))
      if (!byId) continue
      const map: Record<string, Row> = (seed[byId] ??= {})
      if (map[row.id]) Object.assign(map[row.id], structuredClone(row))
      else map[row.id] = structuredClone(row)
    }
  }
  upsert('projects', null, patch.projects)
  upsert('loops', 'loopById', patch.loops)
  upsert('prs', 'prById', patch.prs)
  if (patch.projectGroups) seed.projectGroups = structuredClone(patch.projectGroups)
  if (patch.inbox) seed.inbox = structuredClone(patch.inbox)
  if (patch.settings && seed.settings) Object.assign(seed.settings, structuredClone(patch.settings))
  for (const l of [...(seed.loops ?? []), ...Object.values(seed.loopById ?? {})] as Row[]) {
    if (!l.worktreePath) l.worktreePath = `/mock/worktrees/${l.id}`
  }
}

const FAMILY: Record<string, string> = {
  Inter: 'Inter', SpaceGrotesk: 'Space Grotesk', JetBrainsMono: 'JetBrains Mono',
  IBMPlexMono: 'IBM Plex Mono', SourceCodePro: 'Source Code Pro',
}

function fontsCss(): string {
  return readdirSync(fontsDir)
    .filter((f) => f.endsWith('.ttf'))
    .map((f) => {
      const [, stem, weight] = /^(.+)-(\d+)\.ttf$/.exec(f)!
      return `@font-face{font-family:'${FAMILY[stem]}';font-style:normal;font-weight:${weight};font-display:block;src:url(/__fonts/${f}) format('truetype');}`
    })
    .join('\n')
}

async function run(page: Page, step: Step) {
  if ('move' in step) await page.mouse.move(...step.move)
  else if ('down' in step) {
    await page.mouse.move(...step.down)
    await page.mouse.down()
  } else if ('up' in step) {
    await page.mouse.move(...step.up)
    await page.mouse.up()
  } else if ('click' in step) await page.mouse.click(...step.click)
  else if ('key' in step) await page.keyboard.press(step.key)
  else if ('type' in step) await page.keyboard.type(step.type)
  else if ('wait' in step) await page.waitForTimeout(step.wait)
}

test.beforeAll(() => mkdirSync(out, { recursive: true }))

for (const scene of scenes) {
  if (only && !only.includes(scene.name)) continue
  test(scene.name, async ({ browser }) => {
    const context = await browser.newContext({
      viewport: { width: scene.width, height: scene.height },
      deviceScaleFactor: 2,
    })
    const page = await context.newPage()
    const errors: string[] = []
    page.on('pageerror', (e) => errors.push(e.message))
    await page.route('https://fonts.googleapis.com/**', (r) => r.fulfill({ contentType: 'text/css', body: fontsCss() }))
    await page.route('https://fonts.gstatic.com/**', (r) => r.abort())
    await page.route('**/__fonts/*', (r) =>
      r.fulfill({ contentType: 'font/ttf', body: readFileSync(join(fontsDir, new URL(r.request().url()).pathname.split('/').pop()!)) }),
    )
    const replay = scene.input === undefined ? undefined : spec.terminalInputs?.[scene.input]
    const sceneSeed = structuredClone(seed)
    if (scene.name.startsWith('workspace/')) patchSeed(sceneSeed as Record<string, any>, scene.seed)
    if (scene.name.startsWith('shell/') || scene.name.startsWith('prs/')) {
      patchSeed(sceneSeed as Record<string, any>, scene.name.startsWith('prs/') ? prsFixture(scene).seed : scene.seed)
      // App prefs the shell reads at startup (`initRailCollapsed`, …), and the scene theme.
      const storage = { 'recur.theme': scene.theme, ...(scene.storage ?? {}) }
      await page.addInitScript((items) => {
        for (const [k, v] of Object.entries(items)) localStorage.setItem(k, v)
      }, storage)
    }
    await installBridge(page, sceneSeed, replay !== undefined ? {
      resolvers: { AttachTerminal: () => Buffer.from(replay, 'utf8').toString('base64') },
    } : scene.name.startsWith('review/') ? { resolvers: reviewResolvers(scene.name) }
      : scene.name.startsWith('observatory/') ? { resolvers: observatoryResolvers(scene.name) }
      : scene.name.startsWith('workspace/') ? { resolvers: workspaceResolvers(scene.name) }
      : scene.name.startsWith('prs/') ? { resolvers: prsResolvers(scene) } : {})
    if (scene.name.startsWith('prs/')) {
      // Avatar photos come from the network in the app; neither renderer fetches them here.
      await page.route('https://github.com/**', (r) => r.abort())
      await page.route('https://avatars.githubusercontent.com/**', (r) => r.abort())
    }
    await page.route('**/workspace-file?**', (r) => {
      const body = workspaceFile(r.request().url())
      const mime = /\.pdf(&|$)/.test(r.request().url()) ? 'application/pdf' : 'image/png'
      return body ? r.fulfill({ contentType: mime, body }) : r.fulfill({ status: 404 })
    })
    let extra = scene.treeScroll ? `&treeScroll=${scene.treeScroll}` : ''
    for (const k of ['route', 'pr', 'tab', 'dock', 'seg'] as const) {
      if (scene[k]) extra += `&${k}=${encodeURIComponent(scene[k]!)}`
    }
    await page.goto(`/?scene=${encodeURIComponent(scene.name)}&theme=${scene.theme}${extra}`)
    await page.waitForSelector('[data-scene-ready]')
    // Faces load on first use; wait until every declared face is in (icon fonts included).
    await page.evaluate(async () => {
      await document.fonts.ready
      await Promise.all([...document.fonts].map((f) => f.load().catch(() => undefined)))
      await document.fonts.ready
    })
    for (const step of scene.steps ?? []) await run(page, step)
    // Images (the lightbox's, a message's) must be decoded before the frame is captured.
    await page.evaluate(() => Promise.all([...document.images].map((i) => i.decode().catch(() => undefined))))
    if (replay !== undefined) {
      // xterm draws on the next frames; and the reference must be the WebGL renderer the app
      // ships, not the DOM fallback.
      await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))))
      if (await page.locator('.xterm-rows').count()) throw new Error('xterm fell back to the DOM renderer')
    }
    // Freeze every animation: finite ones at their end (or at `animationTime`), infinite ones
    // at `animationTime` (default 0). The Rust renderer freezes recur-ui motion the same way.
    await page.evaluate((t) => {
      for (const a of document.getAnimations()) {
        a.pause()
        const end = Number(a.effect?.getComputedTiming().endTime ?? 0)
        a.currentTime = t ?? (Number.isFinite(end) ? end : 0)
      }
    }, scene.animationTime ?? null)
    const file = scene.name.replaceAll('/', '__')
    await page.screenshot({ path: join(out, `${file}.png`), caret: 'hide' })
    // Text boxes (CSS px) for the diff's text mask: every rendered text node's client rects.
    const textRects = await page.evaluate(() => {
      const rects: number[][] = []
      const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT)
      for (let n = walker.nextNode(); n; n = walker.nextNode()) {
        if (!n.textContent?.trim()) continue
        const range = document.createRange()
        range.selectNodeContents(n)
        for (const r of range.getClientRects()) if (r.width > 0 && r.height > 0) rects.push([r.x, r.y, r.width, r.height])
      }
      // File-type icons are font glyphs drawn as `::before` content: text, too.
      for (const el of document.querySelectorAll('.fico, [class^="icon "]')) {
        const r = el.getBoundingClientRect()
        if (r.width > 0 && r.height > 0) rects.push([r.x, r.y, r.width, r.height])
      }
      // Placeholders are text with no text node: an empty field's `placeholder` and string
      // `::before` / `::after` content (`.chat-composer.empty:before`). Each covers its first
      // line, measured in its own font.
      const measure = document.createElement('canvas').getContext('2d')!
      const line = (el: Element, s: string, cs: CSSStyleDeclaration, centered: boolean) => {
        const r = el.getBoundingClientRect()
        const box = getComputedStyle(el)
        const left = r.x + parseFloat(box.borderLeftWidth) + parseFloat(box.paddingLeft)
        const top = r.y + parseFloat(box.borderTopWidth) + parseFloat(box.paddingTop)
        const inner = r.height - parseFloat(box.borderTopWidth) - parseFloat(box.borderBottomWidth)
          - parseFloat(box.paddingTop) - parseFloat(box.paddingBottom)
        measure.font = `${cs.fontStyle} ${cs.fontWeight} ${cs.fontSize} ${cs.fontFamily}`
        const lh = parseFloat(cs.lineHeight) || parseFloat(cs.fontSize) * 1.25
        const w = measure.measureText(s).width + s.length * (parseFloat(cs.letterSpacing) || 0)
        if (w > 0) rects.push([left, centered ? top + (inner - lh) / 2 : top, w, lh])
      }
      for (const el of document.querySelectorAll<HTMLInputElement | HTMLTextAreaElement>('input, textarea')) {
        if (!el.value && el.placeholder) line(el, el.placeholder, getComputedStyle(el, '::placeholder'), el.tagName === 'INPUT')
      }
      for (const el of document.body.querySelectorAll('*')) {
        for (const pseudo of ['::before', '::after']) {
          const cs = getComputedStyle(el, pseudo)
          const m = /^"(.{3,})"$/.exec(cs.content)
          if (m && cs.display !== 'none') line(el, m[1], cs, false)
        }
      }
      return rects
    })
    writeFileSync(join(out, `${file}.text.json`), JSON.stringify(textRects))
    if (scene.name === 'text/calibration') {
      const metrics = await page.evaluate(() =>
        [...document.querySelectorAll<HTMLElement>('[data-m]')].map((el) => {
          const r = el.getBoundingClientRect()
          const b = el.querySelector<HTMLElement>('.bl')!.getBoundingClientRect()
          return { id: el.dataset.m, width: r.width, height: r.height, baseline: b.top - r.top }
        }),
      )
      writeFileSync(join(out, 'metrics.json'), JSON.stringify(metrics, null, 1))
    }
    if (process.env.UI_PARITY_BOXES) {
      // Every element's fractional border box (CSS px), to compare layout offsets exactly.
      const boxes = await page.evaluate(() => {
        const fmt = (r: DOMRect, what: string) =>
          `${r.x.toFixed(4)} ${r.y.toFixed(4)} ${r.width.toFixed(4)} ${r.height.toFixed(4)} ${what}`
        const out: string[] = []
        for (const el of document.body.querySelectorAll<HTMLElement>('*')) {
          const r = el.getBoundingClientRect()
          if (r.width === 0 && r.height === 0) continue
          const cls = typeof el.className === 'string' ? el.className : ''
          out.push(fmt(r, `${el.tagName.toLowerCase()}.${cls.split(/\s+/).filter((c) => c && !c.startsWith('svelte-')).join('.')}`))
        }
        const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT)
        for (let n = walker.nextNode(); n; n = walker.nextNode()) {
          if (!n.textContent?.trim()) continue
          const range = document.createRange()
          range.selectNodeContents(n)
          out.push(fmt(range.getBoundingClientRect(), `#text ${n.textContent.trim().slice(0, 40)}`))
        }
        return out
      })
      writeFileSync(join(out, `${file}.boxes.txt`), boxes.join('\n'))
    }
    if (errors.length) throw new Error(`page errors: ${errors.join('; ')}`)
    await context.close()
  })
}
