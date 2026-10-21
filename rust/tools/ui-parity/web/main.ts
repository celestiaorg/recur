import '../../../../frontend/src/styles/app.scss'
import { mount } from 'svelte'
import Scene from './Scene.svelte'

const q = new URLSearchParams(location.search)
const theme = q.get('theme') ?? 'default'
// applyTheme() in lib/stores.ts: Charcoal is the bare :root palette.
if (theme !== 'default') document.documentElement.setAttribute('data-theme', theme)
mount(Scene, { target: document.getElementById('app')!, props: { name: q.get('scene') ?? '' } })
