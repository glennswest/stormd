import { mount } from 'svelte'
import './app.css'
import { initTheme } from './lib/theme.svelte.js'
import App from './App.svelte'

initTheme()

export default mount(App, { target: document.getElementById('app') })
