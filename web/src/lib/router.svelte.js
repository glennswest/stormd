// A hash router in one file. Routes are '#/path/:param' patterns; the current
// match is reactive state any component can read.

const routes = [
  { pattern: '#/', name: 'dashboard' },
  { pattern: '#/logs', name: 'logs' },
  { pattern: '#/terminal', name: 'terminal' },
  { pattern: '#/process/:name', name: 'process' },
  { pattern: '#/ext/:name', name: 'ext' },
  // component ids carry ':' and '/', so the grid root travels in the query:
  // #/grid?id=<component>&rel=<relation>
  { pattern: '#/grid', name: 'grid' },
]

function match(hash) {
  if (!hash || hash === '#') hash = '#/'
  const [path, query] = hash.split('?')
  const params = {}
  for (const r of routes) {
    const rp = r.pattern.split('/')
    const hp = path.split('/')
    if (rp.length !== hp.length) continue
    let ok = true
    for (let i = 0; i < rp.length; i++) {
      if (rp[i].startsWith(':')) params[rp[i].slice(1)] = decodeURIComponent(hp[i])
      else if (rp[i] !== hp[i]) { ok = false; break }
    }
    if (ok) {
      return { name: r.name, params, query: new URLSearchParams(query || '') }
    }
  }
  return { name: 'dashboard', params: {}, query: new URLSearchParams() }
}

export const route = $state({ current: match(location.hash) })

window.addEventListener('hashchange', () => {
  route.current = match(location.hash)
})

export function navigate(hash) {
  location.hash = hash
}
