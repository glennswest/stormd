// Shared live state: the component-summary feed and the plugin nav tabs.
// The feed arrives over /ws/components as full snapshots (with a REST fetch
// for the very first paint), so every consumer always holds a complete
// picture and there is no client-side merging.

import { get, reconnectingSocket } from './api.js'

export const feed = $state({
  components: [],
  connected: false,
  loaded: false,
})

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
