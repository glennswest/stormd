import { mount } from 'svelte'
import 'stormview/themes.css'
import { initTheme } from 'stormview/theme'
import App from './App.svelte'

initTheme()

export default mount(App, { target: document.getElementById('app') })
