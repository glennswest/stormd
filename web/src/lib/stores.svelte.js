// Shared live state: the component-summary feed and the plugin nav tabs.
// The feed arrives over /ws/components as full snapshots (with a REST fetch
// for the very first paint), so every consumer always holds a complete
// picture and there is no client-side merging.

import { get, postJson, reconnectingSocket } from './api.js'
import { setDefaultTheme } from 'stormview/theme'

export const feed = $state({
  components: [],
  connected: false,
  loaded: false,
})

// Auth is a gate in front of the app: `session` says whether a login is
// required and whether this browser already has one. With auth off
// server-side, the gate never appears.
export const auth = $state({
  checked: false,
  required: false,
  authenticated: true,
  user: null,
})

export async function checkAuth() {
  try {
    const s = await get('/api/v1/auth/session')
    auth.required = !!s.required
    auth.authenticated = !!s.authenticated
    auth.user = s.user || null
    // The session endpoint is the one open door, so it also carries what
    // the login screen needs: the instance name and the configured default
    // theme (which yields to this browser's own pick).
    if (s.container) nav.container = s.container
    if (s.theme) setDefaultTheme(s.theme)
  } catch {
    // Can't tell — let the app try; data requests will 401 if auth is on.
  }
  auth.checked = true
}

export async function login(username, password) {
  const r = await postJson('/api/v1/auth/login', { username, password })
  auth.authenticated = true
  auth.user = r.user || username || null
  startFeed()
}

export async function logout() {
  try {
    await postJson('/api/v1/auth/logout', {})
  } catch {}
  location.reload()
}

export const nav = $state({
  container: 'stormd',
  plugins: [],
})

let started = false

export function startFeed() {
  if (started) return
  started = true

  get('/api/v1/components')
    .then((list) => {
      if (!feed.loaded) {
        feed.components = list
        feed.loaded = true
      }
    })
    .catch(() => {})

  reconnectingSocket('/ws/components', {
    onmessage(list) {
      feed.components = list
      feed.loaded = true
    },
    onstatus(s) {
      feed.connected = s === 'open'
    },
  })

  get('/api/v1/cloudid')
    .then((info) => { nav.container = info.container_name || 'stormd' })
    .catch(() => {})

  get('/api/v1/plugins')
    .then((list) => { nav.plugins = list })
    .catch(() => {})
}
