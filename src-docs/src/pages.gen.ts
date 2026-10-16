// deno-fmt-ignore-file
// biome-ignore format: generated types do not need formatting
// prettier-ignore
import type { PathsForPages } from 'waku/router'

// prettier-ignore
type Page =
  | { path: '/bundles/create'; render: 'static' }
  | { path: '/bundles'; render: 'static' }
  | { path: '/bundles/manifest'; render: 'static' }
  | { path: '/bundles/output'; render: 'static' }
  | { path: '/bundles/prompts'; render: 'static' }
  | { path: '/bundles/loops'; render: 'static' }
  | { path: '/bundles/shebang'; render: 'static' }
  | { path: '/bundles/testing'; render: 'static' }
  | { path: '/bundles/typescript'; render: 'static' }
  | { path: '/concepts/chat-context'; render: 'static' }
  | { path: '/concepts/inbox'; render: 'static' }
  | { path: '/concepts/issues'; render: 'static' }
  | { path: '/concepts/loop'; render: 'static' }
  | { path: '/concepts/modes'; render: 'static' }
  | { path: '/concepts/project-toml'; render: 'static' }
  | { path: '/concepts/pull-requests'; render: 'static' }
  | { path: '/concepts/review'; render: 'static' }
  | { path: '/concepts/schedules'; render: 'static' }
  | { path: '/concepts/workspaces'; render: 'static' }
  | { path: '/getting-started/first-workspace'; render: 'static' }
  | { path: '/getting-started/install'; render: 'static' }
  | { path: '/'; render: 'static' }
  | { path: '/reference/agents'; render: 'static' }
  | { path: '/reference/integrations'; render: 'static' }
  | { path: '/reference/settings'; render: 'static' }
  | { path: '/reference/shortcuts'; render: 'static' }
  | { path: '/reference/skills'; render: 'static' }
  | { path: '/reference/themes'; render: 'static' }
  | { path: '/reference/troubleshooting'; render: 'static' }

// prettier-ignore
declare module 'waku/router' {
  interface RouteConfig {
    paths: PathsForPages<Page>
  }
  interface CreatePagesConfig {
    pages: Page
  }
}
