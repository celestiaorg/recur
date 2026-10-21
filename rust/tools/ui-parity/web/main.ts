import '../../../../frontend/src/styles/app.scss'
import { mount } from 'svelte'
import { setTheme } from '../../../../frontend/src/lib/stores'
import Scene from './Scene.svelte'

const q = new URLSearchParams(location.search)
// The app's own theme switch: sets data-theme and the theme store file icons read.
setTheme(q.get('theme') ?? 'default')
mount(Scene, { target: document.getElementById('app')!, props: { name: q.get('scene') ?? '' } })
