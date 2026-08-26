<script>
  // RSS/VMS history sparkline, drawn on canvas from /api/v1/memory/history.
  import { onMount } from 'svelte'
  import { get, formatBytes } from '../api.js'

  let canvas = $state(null)
  let current = $state(null)

  async function refresh() {
    try {
      const [stats, history] = await Promise.all([
        get('/api/v1/stats'),
        get('/api/v1/memory/history'),
      ])
      current = stats.memory || null
      draw(history || [])
    } catch {
      /* endpoint unavailable — leave the last drawing */
    }
  }

  function draw(samples) {
    if (!canvas) return
    const ctx = canvas.getContext('2d')
    const dpr = window.devicePixelRatio || 1
    const rect = canvas.parentElement.getBoundingClientRect()
    canvas.width = rect.width * dpr
    canvas.height = rect.height * dpr
    ctx.scale(dpr, dpr)
    const W = rect.width
    const H = rect.height

    ctx.clearRect(0, 0, W, H)
    if (samples.length < 2) {
      ctx.fillStyle = '#666'
      ctx.font = '12px system-ui'
      ctx.fillText('Collecting data…', W / 2 - 50, H / 2)
      return
    }

    const maxRss = Math.max(...samples.map((s) => s.rss_bytes)) * 1.1 || 1
    const maxVms = Math.max(...samples.map((s) => s.vms_bytes)) * 1.1 || 1

    ctx.strokeStyle = '#1a1d32'
    ctx.lineWidth = 1
    ctx.font = '10px system-ui'
    for (let i = 0; i <= 4; i++) {
      const y = H - (i / 4) * (H - 20)
      ctx.beginPath()
      ctx.moveTo(40, y)
      ctx.lineTo(W, y)
      ctx.stroke()
      ctx.fillStyle = '#555'
      ctx.fillText(formatBytes((maxRss * i) / 4), 0, y + 3)
    }

    const first = new Date(samples[0].timestamp)
    const last = new Date(samples[samples.length - 1].timestamp)
    ctx.fillStyle = '#555'
    ctx.fillText(first.toLocaleTimeString(), 40, H - 2)
    ctx.fillText(last.toLocaleTimeString(), W - 60, H - 2)

    const plot = (values, max, style, width) => {
      ctx.strokeStyle = style
      ctx.lineWidth = width
      ctx.beginPath()
      values.forEach((v, i) => {
        const x = 40 + (i / (values.length - 1)) * (W - 44)
        const y = H - 20 - (v / max) * (H - 40) + 10
        if (i === 0) ctx.moveTo(x, y)
        else ctx.lineTo(x, y)
      })
      ctx.stroke()
    }
    plot(samples.map((s) => s.rss_bytes), maxRss, '#50fa7b', 2)
    plot(samples.map((s) => s.vms_bytes), maxVms, 'rgba(139,233,253,0.3)', 1)

    ctx.fillStyle = '#50fa7b'
    ctx.fillRect(W - 100, 6, 10, 3)
    ctx.fillStyle = '#888'
    ctx.fillText('RSS', W - 86, 12)
    ctx.fillStyle = 'rgba(139,233,253,0.5)'
    ctx.fillRect(W - 55, 6, 10, 3)
    ctx.fillStyle = '#888'
    ctx.fillText('VMS', W - 41, 12)
  }

  onMount(() => {
    refresh()
    const t = setInterval(refresh, 5000)
    return () => clearInterval(t)
  })
</script>

<div class="card">
  <h2>Memory</h2>
  <div class="current">
    {#if current}
      RSS <span class="rss">{formatBytes(current.rss_bytes)}</span>
      &nbsp; VMS <span class="vms">{formatBytes(current.vms_bytes)}</span>
    {:else}
      <span class="na">Memory info not available</span>
    {/if}
  </div>
  <div class="chart-wrap"><canvas bind:this={canvas}></canvas></div>
</div>

<style>
  .card {
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 14px 16px;
  }
  h2 {
    font-size: 12px;
    font-weight: 600;
    color: var(--text-dim);
    margin-bottom: 8px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }
  .current { font-size: 13px; margin-bottom: 8px; }
  .rss { color: var(--ok); font-weight: 600; }
  .vms { color: var(--accent); }
  .na { color: var(--text-faint); }
  .chart-wrap { position: relative; width: 100%; height: 160px; }
  canvas { width: 100%; height: 100%; }
</style>
