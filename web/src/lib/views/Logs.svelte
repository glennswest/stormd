<script>
  // Live log tail over /ws/logs plus stored-run and local-archive browsing.
  // Entries append imperatively into the pane — at thousands of lines a
  // keyed list re-render is the wrong tool.
  import { onMount } from 'svelte'
  import { route } from '../router.svelte.js'
  import { get, wsUrl, ansiToHtml, escapeHtml, formatBytes } from '../api.js'

  let processes = $state([])
  let runs = $state([])
  let process = $state('')
  let run = $state('') // '' = live, 'local:<file>' = archive, else run_id
  let severity = $state('info')
  let stream = $state('')
  let search = $state('')
  let follow = $state(true)
  let count = $state(0)
  let runInfo = $state('')

  let pane = $state(null)
  let ws = null

  const live = $derived(run === '')

  function clearPane() {
    if (pane) pane.innerHTML = ''
    count = 0
  }

  function sevColor(sev) {
    const s = (sev || '').toLowerCase()
    if (s === 'error' || s === 'critical' || s === 'emergency' || s === 'alert') return 'color:var(--error)'
    if (s === 'warning' || s === 'warn') return 'color:var(--warn-strong)'
    if (s === 'debug') return 'color:var(--text-ghost)'
    return ''
  }

  function appendEntry(entry, dim = false) {
    if (!pane) return
    if (search && !(entry.line || '').includes(search)) return
    if (stream && (entry.stream || '') !== stream) return

    const scrollBefore = pane.scrollTop
    const ts = entry.timestamp ? new Date(entry.timestamp).toLocaleTimeString() : ''
    const style = dim ? 'color:var(--text-ghost)' : sevColor(entry.severity)
    pane.insertAdjacentHTML(
      'beforeend',
      `<div class="log-entry" style="${style}">` +
        (ts ? `<span style="color:var(--text-faint)">${escapeHtml(ts)}</span> ` : '') +
        `<span style="color:var(--accent)">${escapeHtml(entry.process || '')}</span> ` +
        `<span style="color:var(--text-ghost)">[${escapeHtml(entry.stream || '')}]</span> ` +
        ansiToHtml(entry.line || '') +
        '</div>'
    )
    count++
    if (follow) pane.scrollTop = pane.scrollHeight
    else pane.scrollTop = scrollBefore
    while (pane.children.length > 5000) {
      pane.removeChild(pane.firstChild)
      count--
    }
  }

  function appendRaw(line, style = '') {
    if (!pane) return
    pane.insertAdjacentHTML(
      'beforeend',
      `<div class="log-entry" style="${style}">${ansiToHtml(line)}</div>`
    )
    count++
  }

  function connectWs() {
    ws?.close()
    runInfo = ''
    let url = '/ws/logs?'
    if (process) url += 'process=' + encodeURIComponent(process) + '&'
    if (severity) url += 'severity=' + encodeURIComponent(severity)
    ws = new WebSocket(wsUrl(url))
    ws.onmessage = (e) => {
      try {
        appendEntry(JSON.parse(e.data))
      } catch {}
    }
  }

  async function loadRecent() {
    if (!process) return
    try {
      const data = await get('/api/v1/logs/' + encodeURIComponent(process) + '?tail=100')
      for (const line of data.lines || []) appendRaw(line, 'color:var(--text-ghost)')
      if ((data.lines || []).length) {
        appendRaw('--- recent history above, live stream below ---', 'color:var(--text-faint);font-size:11px')
        if (follow && pane) pane.scrollTop = pane.scrollHeight
      }
    } catch {}
  }

  async function loadProcesses() {
    try {
      processes = await get('/api/v1/processes')
    } catch {}
  }

  function formatRunId(rid) {
    if (rid && rid.length === 15) {
      return (
        rid.substring(0, 4) + '-' + rid.substring(4, 6) + '-' + rid.substring(6, 8) +
        ' ' + rid.substring(9, 11) + ':' + rid.substring(11, 13) + ':' + rid.substring(13, 15)
      )
    }
    return rid || ''
  }

  async function loadRuns() {
    runs = []
    if (!process) return
    const found = []
    try {
      const data = await get('/api/v1/logs/' + encodeURIComponent(process) + '/runs')
      const currentRunId = data.current_run_id
      if (currentRunId) {
        found.push({ value: currentRunId, label: formatRunId(currentRunId) + ' (current)' })
      }
      for (const r of data.runs || []) {
        if (r.run_id === currentRunId) continue
        found.push({
          value: r.run_id,
          label: formatRunId(r.run_id) + ' — ' + (r.date || '') + ' (' + formatBytes(r.size_bytes) + ')',
        })
      }
    } catch {}
    try {
      const files = await get('/api/v1/logs/files')
      for (const f of files || []) {
        const m = (f.name || '').match(
          new RegExp('^' + process.replace(/[.*+?^$|[\]\\{}]/g, '\\$&') + '\\.(\\d{8}T\\d{6})\\.(failed|exited)\\.log$')
        )
        if (m && !found.some((o) => o.value === m[1])) {
          found.push({
            value: 'local:' + f.name,
            label: formatRunId(m[1]) + ' [' + m[2] + '] (local, ' + formatBytes(f.size_bytes) + ')',
            failed: m[2] === 'failed',
          })
        }
      }
    } catch {}
    runs = found
  }

  async function loadStoredRun(runId) {
    runInfo = 'Loading run ' + formatRunId(runId) + ' …'
    try {
      let url = '/api/v1/logs/stored?process=' + encodeURIComponent(process) + '&run_id=' + encodeURIComponent(runId)
      if (search) url += '&search=' + encodeURIComponent(search)
      const entries = await get(url)
      clearPane()
      for (const e of entries) appendEntry(e)
      runInfo =
        'Run ' + formatRunId(runId) + ' — ' + entries.length + ' entries' +
        (entries.length
          ? ' — ' + new Date(entries[0].timestamp).toLocaleString() + ' to ' +
            new Date(entries[entries.length - 1].timestamp).toLocaleString()
          : '')
    } catch (e) {
      runInfo = 'Failed to load run: ' + e.message
    }
  }

  async function loadLocalFile(filename) {
    runInfo = 'Loading archive ' + filename + ' …'
    try {
      const data = await get('/api/v1/logs/files/' + encodeURIComponent(filename) + '?tail=10000')
      clearPane()
      for (const line of data.lines || []) appendRaw(line)
      runInfo =
        'Archive ' + filename + (filename.includes('.failed.') ? ' [FAILED]' : ' [EXITED]') +
        ' — ' + count + ' lines'
    } catch (e) {
      runInfo = 'Failed to load: ' + e.message
    }
  }

  function refresh() {
    clearPane()
    if (live) {
      connectWs()
      loadRecent()
    } else {
      ws?.close()
      ws = null
      if (run.startsWith('local:')) loadLocalFile(run.substring(6))
      else loadStoredRun(run)
    }
  }

  function onProcessChange() {
    run = ''
    loadRuns()
    refresh()
  }

  // Crash links from a component card arrive as #/logs?process=x&show=crash&ts=…
  // Pick the failed run that started closest before the restart timestamp.
  async function applyQuery() {
    const q = route.current.query
    const preselect = q.get('process')
    if (!preselect) return false
    process = preselect
    await loadRuns()
    if (q.get('show') === 'crash') {
      const restartTime = q.get('ts') ? new Date(q.get('ts')).getTime() : 0
      let best = null
      let bestDist = Infinity
      for (const opt of runs) {
        if (!opt.failed && !(opt.label || '').includes('failed')) continue
        const m = opt.value.replace('local:', '').match(/(\d{8}T\d{6})/)
        if (m) {
          const rid = m[1]
          const runDate = new Date(
            rid.substring(0, 4) + '-' + rid.substring(4, 6) + '-' + rid.substring(6, 8) +
            'T' + rid.substring(9, 11) + ':' + rid.substring(11, 13) + ':' + rid.substring(13, 15) + 'Z'
          )
          const dist = restartTime - runDate.getTime()
          if (dist >= 0 && dist < bestDist) {
            bestDist = dist
            best = opt
          }
        }
        if (!best) best = opt
      }
      if (best) {
        run = best.value
        refresh()
        return true
      }
    }
    return false
  }

  onMount(() => {
    loadProcesses()
    applyQuery().then((handled) => {
      if (!handled) refresh()
    })
    const t = setInterval(loadProcesses, 10000)
    return () => {
      clearInterval(t)
      ws?.close()
    }
  })
</script>

<div class="toolbar">
  <label>Process
    <select bind:value={process} onchange={onProcessChange}>
      <option value="">All</option>
      {#each processes as p}
        <option value={p.name}>{p.name}</option>
      {/each}
    </select>
  </label>
  <label>Run
    <select bind:value={run} onchange={refresh} disabled={!process}>
      <option value="">Live</option>
      {#each runs as r}
        <option value={r.value}>{r.label}</option>
      {/each}
    </select>
  </label>
  <label>Severity
    <select bind:value={severity} onchange={() => live && refresh()}>
      <option value="">All</option>
      <option value="emergency">Emergency</option>
      <option value="critical">Critical</option>
      <option value="error">Error</option>
      <option value="warning">Warning</option>
      <option value="info">Info+</option>
      <option value="debug">Debug</option>
    </select>
  </label>
  <label>Stream
    <select bind:value={stream} onchange={refresh}>
      <option value="">All</option>
      <option value="stdout">stdout</option>
      <option value="stderr">stderr</option>
    </select>
  </label>
  <input
    type="search"
    placeholder="Search…"
    bind:value={search}
    onkeyup={(e) => e.key === 'Enter' && refresh()}
  />
  {#if live}
    <label class="cb"><input type="checkbox" bind:checked={follow} /> Follow</label>
  {/if}
  <button onclick={clearPane}>Clear</button>
  <span class="count">{count} entries</span>
</div>

{#if runInfo}
  <div class="run-info">{runInfo}</div>
{/if}

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
    flex-wrap: wrap;
  }
  .toolbar label {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    color: var(--text-dim);
  }
  .toolbar input[type='search'] { width: 200px; }
  label.cb { cursor: pointer; }
  label.cb input { accent-color: var(--ok); }
  .count { font-size: 12px; color: var(--text-faint); margin-left: auto; }
  .run-info {
    font-size: 11px;
    color: var(--text-faint);
    padding: 8px 20px;
    border-bottom: 1px solid var(--panel-raised);
  }
  .pane-wrap { padding-top: 8px; }
  .pane { max-height: calc(100vh - 170px); min-height: 300px; }
</style>
