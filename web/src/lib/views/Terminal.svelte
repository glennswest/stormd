<script>
  // Live VT100 view of a process over /ws/console/{process}: an initial
  // screen snapshot, then streamed entries.
  import { onMount } from 'svelte'
  import { get, wsUrl, ansiToHtml, escapeHtml } from '../api.js'

  let { preselect = '' } = $props()

  let processes = $state([])
  let process = $state(preselect || '')
  let status = $state('disconnected')
  let pane = $state(null)
  let ws = null

  async function loadProcesses() {
    try {
      processes = await get('/api/v1/processes')
      if (!process && processes.length > 0) {
        process = processes[0].name
        connect()
      }
    } catch {}
  }

  function append(html) {
    if (!pane) return
    pane.insertAdjacentHTML('beforeend', html)
    pane.scrollTop = pane.scrollHeight
    while (pane.children.length > 5000) pane.removeChild(pane.firstChild)
  }

  function connect() {
    ws?.close()
    if (pane) pane.innerHTML = ''
    if (!process) return
    status = 'connecting'
    ws = new WebSocket(wsUrl('/ws/console/' + encodeURIComponent(process)))
    ws.onopen = () => (status = 'connected')
    ws.onmessage = (e) => {
      const msg = JSON.parse(e.data)
      if (msg.type === 'snapshot') {
        if (pane)
          pane.innerHTML =
            '<div style="color:var(--text-faint)">--- terminal snapshot ---</div>' +
            ansiToHtml(msg.data.contents || '') +
            '<div style="color:var(--text-faint)">--- live output ---</div>\n'
      } else if (msg.type === 'entry') {
        const cls = msg.data.stream || 'stdout'
        const ts = new Date(msg.data.timestamp).toLocaleTimeString()
        const color = cls === 'stderr' ? 'color:var(--error)' : ''
        append(
          `<div class="log-entry" style="${color}"><span style="color:var(--text-faint)">${escapeHtml(ts)}</span> ` +
            `<span style="color:var(--text-ghost)">[${escapeHtml(cls)}]</span> ${ansiToHtml(msg.data.line || '')}</div>`
        )
      } else if (msg.type === 'lagged') {
        append(`<div style="color:var(--warn-strong)">--- skipped ${msg.skipped} entries ---</div>`)
      }
    }
    ws.onclose = () => {
      status = 'disconnected'
      append('<div style="color:var(--text-faint)">--- disconnected ---</div>')
    }
  }

  onMount(() => {
    if (process) connect()
    loadProcesses()
    const t = setInterval(loadProcesses, 10000)
    return () => {
      clearInterval(t)
      ws?.close()
    }
  })
</script>

<div class="toolbar">
  <label>Process
    <select bind:value={process} onchange={connect}>
      {#each processes as p}
        <option value={p.name}>{p.name} ({(p.state || '').toLowerCase()})</option>
      {/each}
    </select>
  </label>
  <span class="badge {status}">{status}</span>
</div>

<div class="content pane-wrap">
  <div class="term-output pane" bind:this={pane}></div>
</div>

<style>
  .toolbar {
    padding: 10px 20px;
    display: flex;
    gap: 12px;
    align-items: center;
    border-bottom: 1px solid var(--border);
  }
  .toolbar label {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    color: var(--text-dim);
  }
  .badge {
    padding: 2px 10px;
    border-radius: 10px;
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    background: var(--panel-raised);
    color: var(--text-dim);
  }
  .badge.connected { background: var(--ok-bg); color: var(--ok); }
  .badge.connecting { background: var(--warn-bg); color: var(--warn); }
  .badge.disconnected { background: var(--error-bg); color: var(--error); }
  .pane-wrap { padding-top: 8px; }
  .pane { max-height: calc(100vh - 160px); min-height: 300px; }
</style>
