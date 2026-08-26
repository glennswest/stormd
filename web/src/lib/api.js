// REST + WebSocket helpers, and the formatting/ANSI utilities shared by every
// view. All server communication goes through here.

export async function get(path) {
  const resp = await fetch(path)
  if (!resp.ok) throw new Error(`${resp.status} ${resp.statusText}`)
  return resp.json()
}

export async function post(path) {
  const resp = await fetch(path, { method: 'POST' })
  if (!resp.ok) throw new Error(`${resp.status} ${resp.statusText}`)
  return resp.json()
}

export function wsUrl(path) {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${proto}//${location.host}${path}`
}

/// A WebSocket that redials itself. Returns a handle with close(); onmessage
/// receives parsed JSON, onstatus receives 'connecting' | 'open' | 'closed'.
export function reconnectingSocket(path, { onmessage, onstatus } = {}) {
  let ws = null
  let closed = false
  let delay = 500

  function dial() {
    if (closed) return
    onstatus?.('connecting')
    ws = new WebSocket(wsUrl(path))
    ws.onopen = () => {
      delay = 500
      onstatus?.('open')
    }
    ws.onmessage = (e) => {
      try {
        onmessage?.(JSON.parse(e.data))
      } catch {
        /* non-JSON frame — ignore */
      }
    }
    ws.onclose = () => {
      onstatus?.('closed')
      if (!closed) {
        setTimeout(dial, delay)
        delay = Math.min(delay * 2, 10000)
      }
    }
  }

  dial()
  return {
    close() {
      closed = true
      ws?.close()
    },
  }
}

// --- formatting ---

export function formatBytes(bytes) {
  if (bytes == null) return '-'
  if (bytes === 0) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1)
  const val = bytes / Math.pow(1024, i)
  return val.toFixed(i > 0 ? 1 : 0) + ' ' + units[i]
}

export function formatDuration(secs) {
  if (secs == null) return '-'
  const d = Math.floor(secs / 86400)
  const h = Math.floor((secs % 86400) / 3600)
  const m = Math.floor((secs % 3600) / 60)
  const s = Math.floor(secs % 60)
  if (d > 0) return `${d}d ${h}h`
  if (h > 0) return `${h}h ${m}m`
  if (m > 0) return `${m}m ${s}s`
  return `${s}s`
}

export function timeAgo(ts) {
  if (!ts) return '-'
  const diff = (Date.now() - new Date(ts).getTime()) / 1000
  if (diff < 60) return Math.floor(diff) + 's ago'
  if (diff < 3600) return Math.floor(diff / 60) + 'm ago'
  if (diff < 86400) return Math.floor(diff / 3600) + 'h ago'
  return Math.floor(diff / 86400) + 'd ago'
}

// --- ANSI → HTML ---

export function escapeHtml(s) {
  return String(s)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
}

const ANSI_STYLES = {
  1: 'font-weight:bold',
  2: 'opacity:0.7',
  3: 'font-style:italic',
  4: 'text-decoration:underline',
  30: 'color:#555',
  31: 'color:#e94560',
  32: 'color:#50fa7b',
  33: 'color:#f1fa8c',
  34: 'color:#6272a4',
  35: 'color:#ff79c6',
  36: 'color:#8be9fd',
  37: 'color:#ccc',
  90: 'color:#666',
  91: 'color:#ff6e6e',
  92: 'color:#69ff94',
  93: 'color:#ffffa5',
  94: 'color:#d6acff',
  95: 'color:#ff92df',
  96: 'color:#a4ffff',
  97: 'color:#fff',
}

export function ansiToHtml(text) {
  text = String(text)
    .replace(/\x1b\[\d*[ABCDHJ]/g, '')
    .replace(/\x1b\[\d*;\d*[Hf]/g, '')
    .replace(/\x1b\[\??\d*[hlr]/g, '')

  let result = ''
  let openSpans = 0
  const parts = text.split(/\x1b\[/)

  result += escapeHtml(parts[0])
  for (let i = 1; i < parts.length; i++) {
    const match = parts[i].match(/^([\d;]*)m([\s\S]*)/)
    if (match) {
      const codes = match[1]
      const rest = match[2]
      if (codes === '0' || codes === '') {
        while (openSpans > 0) { result += '</span>'; openSpans-- }
      } else {
        const styles = []
        for (const code of codes.split(';')) {
          if (code === '0') {
            while (openSpans > 0) { result += '</span>'; openSpans-- }
          } else if (ANSI_STYLES[code]) {
            styles.push(ANSI_STYLES[code])
          }
        }
        if (styles.length > 0) {
          result += '<span style="' + styles.join(';') + '">'
          openSpans++
        }
      }
      result += escapeHtml(rest)
    } else {
      result += '\x1b[' + escapeHtml(parts[i])
    }
  }
  while (openSpans > 0) { result += '</span>'; openSpans-- }
  return result
}
