// Theme selection: sets data-theme on <html> (the token overrides in app.css
// do the rest) and remembers the choice in localStorage per browser.

export const THEMES = [
  { id: 'storm', label: 'Storm' },
  { id: 'midnight', label: 'Midnight' },
  { id: 'nord', label: 'Nord' },
  { id: 'solar', label: 'Solar' },
  { id: 'phosphor', label: 'Phosphor' },
  { id: 'light', label: 'Light' },
]

const KEY = 'stormd-theme'

function stored() {
  try {
    const t = localStorage.getItem(KEY)
    return THEMES.some((x) => x.id === t) ? t : 'storm'
  } catch {
    return 'storm'
  }
}

export const theme = $state({ current: stored() })

export function applyTheme(id) {
  theme.current = id
  if (id === 'storm') delete document.documentElement.dataset.theme
  else document.documentElement.dataset.theme = id
  try {
    localStorage.setItem(KEY, id)
  } catch {}
}

export function initTheme() {
  applyTheme(theme.current)
}
