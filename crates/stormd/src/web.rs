use crate::api::AppState;
use axum::extract::State;
use axum::response::Html;
use std::sync::Arc;

/// Serve the main dashboard page.
pub async fn dashboard_page(State(state): State<Arc<AppState>>) -> Html<String> {
    Html(build_dashboard(&state.container_name))
}

/// Serve the web terminal page.
pub async fn terminal_page(State(state): State<Arc<AppState>>) -> Html<String> {
    Html(build_terminal(&state.container_name))
}

/// Serve the log viewer page.
pub async fn logs_page(State(state): State<Arc<AppState>>) -> Html<String> {
    Html(build_logs(&state.container_name))
}

// --- Shared CSS + JS ---

fn nav_css() -> &'static str {
    r#"
* { margin: 0; padding: 0; box-sizing: border-box; }
body { background: #0f0f1a; color: #e0e0e0; font-family: -apple-system, 'Segoe UI', system-ui, sans-serif; }
a { color: #8be9fd; text-decoration: none; }
a:hover { text-decoration: underline; }

nav { background: #16192e; border-bottom: 1px solid #2a2d45; padding: 0 20px; display: flex; align-items: center; height: 48px; }
nav .brand { font-size: 18px; font-weight: 700; color: #e94560; margin-right: 32px; letter-spacing: -0.5px; }
nav .links { display: flex; gap: 4px; }
nav .links a {
    padding: 8px 16px; border-radius: 6px; font-size: 13px; font-weight: 500;
    color: #888; transition: all 0.15s;
}
nav .links a:hover { color: #e0e0e0; background: #1e2140; text-decoration: none; }
nav .links a.active { color: #fff; background: #2a2d50; }

.controls { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
select, input[type="text"], input[type="search"] {
    background: #1a1d32; color: #e0e0e0; border: 1px solid #2a2d45; padding: 6px 12px;
    border-radius: 6px; font-size: 13px; outline: none;
}
select:focus, input:focus { border-color: #4a4d70; }
button {
    background: #2a2d50; color: #e0e0e0; border: 1px solid #3a3d60; padding: 6px 14px;
    border-radius: 6px; font-size: 13px; cursor: pointer; transition: all 0.15s;
}
button:hover { background: #3a3d60; }
button.btn-green { background: #1a4a2a; border-color: #2a6a3a; color: #50fa7b; }
button.btn-green:hover { background: #2a5a3a; }
button.btn-red { background: #4a1a2a; border-color: #6a2a3a; color: #e94560; }
button.btn-red:hover { background: #5a2a3a; }
button.btn-yellow { background: #4a3a1a; border-color: #6a5a2a; color: #f1fa8c; }
button.btn-yellow:hover { background: #5a4a2a; }

.badge {
    display: inline-block; padding: 2px 8px; border-radius: 10px; font-size: 11px;
    font-weight: 600; text-transform: uppercase; letter-spacing: 0.5px;
}
.badge-green { background: #1a4a2a; color: #50fa7b; }
.badge-red { background: #4a1a2a; color: #e94560; }
.badge-yellow { background: #4a3a1a; color: #f1fa8c; }
.badge-cyan { background: #1a3a4a; color: #8be9fd; }
.badge-gray { background: #2a2d45; color: #888; }

table { width: 100%; border-collapse: collapse; }
th { text-align: left; padding: 10px 12px; font-size: 11px; font-weight: 600; text-transform: uppercase;
     letter-spacing: 0.5px; color: #666; border-bottom: 1px solid #2a2d45; }
td { padding: 10px 12px; font-size: 13px; border-bottom: 1px solid #1a1d32; }
tr:hover { background: #1a1d32; }

.card {
    background: #16192e; border: 1px solid #2a2d45; border-radius: 8px;
    padding: 16px 20px; margin-bottom: 16px;
}
.card h2 { font-size: 14px; font-weight: 600; color: #888; margin-bottom: 12px; text-transform: uppercase; letter-spacing: 0.5px; }

.mono { font-family: 'SF Mono', 'Fira Code', 'Cascadia Code', monospace; }
.content { padding: 16px 20px; }

.stats-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(160px, 1fr)); gap: 12px; margin-bottom: 16px; }
.stat-card {
    background: #16192e; border: 1px solid #2a2d45; border-radius: 8px; padding: 16px;
}
.stat-card .label { font-size: 11px; color: #666; text-transform: uppercase; letter-spacing: 0.5px; margin-bottom: 4px; }
.stat-card .value { font-size: 24px; font-weight: 700; }
.stat-card .value.green { color: #50fa7b; }
.stat-card .value.red { color: #e94560; }
.stat-card .value.yellow { color: #f1fa8c; }
.stat-card .value.cyan { color: #8be9fd; }

.term-output {
    background: #0a0a14; border: 1px solid #2a2d45; border-radius: 8px; padding: 12px;
    font-family: 'SF Mono', 'Fira Code', 'Cascadia Code', monospace;
    font-size: 13px; line-height: 1.5; overflow-y: auto; white-space: pre-wrap;
    max-height: calc(100vh - 200px); min-height: 300px;
}
.log-entry { padding: 1px 0; }
"#
}

fn ansi_js() -> &'static str {
    r#"
function escapeHtml(s) {
    return s.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');
}

function ansiToHtml(text) {
    const colorMap = {
        '1': 'font-weight:bold',
        '2': 'opacity:0.7',
        '3': 'font-style:italic',
        '4': 'text-decoration:underline',
        '30': 'color:#555',
        '31': 'color:#e94560',
        '32': 'color:#50fa7b',
        '33': 'color:#f1fa8c',
        '34': 'color:#6272a4',
        '35': 'color:#ff79c6',
        '36': 'color:#8be9fd',
        '37': 'color:#ccc',
        '90': 'color:#666',
        '91': 'color:#ff6e6e',
        '92': 'color:#69ff94',
        '93': 'color:#ffffa5',
        '94': 'color:#d6acff',
        '95': 'color:#ff92df',
        '96': 'color:#a4ffff',
        '97': 'color:#fff',
    };

    text = text.replace(/\x1b\[\d*[ABCDHJ]/g, '');
    text = text.replace(/\x1b\[\d*;\d*[Hf]/g, '');
    text = text.replace(/\x1b\[\??\d*[hlr]/g, '');

    let result = '';
    let openSpans = 0;
    const parts = text.split(/\x1b\[/);

    result += escapeHtml(parts[0]);
    for (let i = 1; i < parts.length; i++) {
        const match = parts[i].match(/^([\d;]*)m([\s\S]*)/);
        if (match) {
            const codes = match[1];
            const rest = match[2];
            if (codes === '0' || codes === '') {
                while (openSpans > 0) { result += '</span>'; openSpans--; }
            } else {
                const styles = [];
                for (const code of codes.split(';')) {
                    if (code === '0') {
                        while (openSpans > 0) { result += '</span>'; openSpans--; }
                    } else if (colorMap[code]) {
                        styles.push(colorMap[code]);
                    }
                }
                if (styles.length > 0) {
                    result += '<span style="' + styles.join(';') + '">';
                    openSpans++;
                }
            }
            result += escapeHtml(rest);
        } else {
            result += '\x1b[' + escapeHtml(parts[i]);
        }
    }
    while (openSpans > 0) { result += '</span>'; openSpans--; }
    return result;
}
"#
}

fn nav_html(active: &str, container_name: &str) -> String {
    let pages = [("Dashboard", "/ui/"), ("Terminal", "/ui/terminal"), ("Logs", "/ui/logs")];
    let links: Vec<String> = pages
        .iter()
        .map(|(name, href)| {
            let cls = if *name == active { " class=\"active\"" } else { "" };
            format!("<a href=\"{}\"{}>{}</a>", href, cls, name)
        })
        .collect();
    format!(
        "<nav><span class=\"brand\">{}</span><div class=\"links\">{}</div><span style=\"margin-left:auto;font-size:12px;color:#888;font-weight:500\">stormd</span></nav>",
        container_name, links.join("")
    )
}

// --- Dashboard ---

fn build_dashboard(name: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html>
<head>
    <title>stormd</title>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <style>
{css}
    .process-actions {{ display: flex; gap: 4px; }}
    .process-actions button {{ padding: 4px 10px; font-size: 12px; }}
    .two-col {{ display: grid; grid-template-columns: 1fr 1fr; gap: 16px; }}
    @media (max-width: 900px) {{ .two-col {{ grid-template-columns: 1fr; }} }}
    .chart-wrap {{ position: relative; width: 100%; height: 180px; }}
    .chart-wrap canvas {{ width: 100% !important; height: 100% !important; }}
    .usage-bar {{ background: #1a1d32; border-radius: 4px; height: 18px; overflow: hidden; position: relative; }}
    .usage-bar-fill {{ height: 100%; border-radius: 4px; transition: width 0.3s; }}
    .usage-bar-text {{ position: absolute; top: 0; left: 8px; right: 8px; height: 18px; line-height: 18px; font-size: 11px; color: #ccc; }}
    .restart-list {{ max-height: 120px; overflow-y: auto; font-size: 12px; color: #888; }}
    .restart-list div {{ padding: 2px 0; border-bottom: 1px solid #1a1d32; }}
    </style>
</head>
<body>
    {nav}
    <div class="content">
        <div class="stats-grid" id="stats"></div>

        <div class="card">
            <h2>Processes</h2>
            <table>
                <thead>
                    <tr>
                        <th>Name</th>
                        <th>State</th>
                        <th>PID</th>
                        <th>Exit Code</th>
                        <th>Crashes</th>
                        <th>Restarts</th>
                        <th>Last Restart</th>
                        <th>Uptime</th>
                        <th>Actions</th>
                    </tr>
                </thead>
                <tbody id="procs"></tbody>
            </table>
        </div>

        <div class="two-col">
            <div class="card">
                <h2>Memory Usage</h2>
                <div id="mem-current" style="margin-bottom:8px;font-size:13px"></div>
                <div class="chart-wrap"><canvas id="mem-chart"></canvas></div>
            </div>
            <div class="card">
                <h2>Disk / Mounts</h2>
                <div id="mounts"></div>
            </div>
        </div>

        <div class="two-col">
            <div class="card" id="restart-section" style="display:none">
                <h2>Restart History</h2>
                <div id="restart-history"></div>
            </div>
            <div class="card" id="cron-section" style="display:none">
                <h2>Cron Jobs</h2>
                <table>
                    <thead>
                        <tr>
                            <th>Name</th>
                            <th>Schedule</th>
                            <th>Runs</th>
                            <th>Failures</th>
                            <th>Next Run</th>
                        </tr>
                    </thead>
                    <tbody id="crons"></tbody>
                </table>
            </div>
        </div>

        <div class="card" id="updates-section" style="display:none">
            <h2>Image Updates</h2>
            <table>
                <thead>
                    <tr>
                        <th>Process</th>
                        <th>Image</th>
                        <th>Current Digest</th>
                        <th>Status</th>
                        <th>Actions</th>
                    </tr>
                </thead>
                <tbody id="updates"></tbody>
            </table>
        </div>
    </div>

    <script>
{ansi_js}

    function stateBadge(state) {{
        const s = (typeof state === 'string' ? state : state || '').toLowerCase();
        const cls = s === 'running' ? 'badge-green' : s === 'failed' ? 'badge-red' :
                    s === 'stopped' ? 'badge-yellow' : s === 'restarting' ? 'badge-cyan' : 'badge-gray';
        return `<span class="badge ${{cls}}">${{escapeHtml(s)}}</span>`;
    }}

    function formatBytes(bytes) {{
        if (bytes == null || bytes === 0) return '0 B';
        const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
        const i = Math.floor(Math.log(bytes) / Math.log(1024));
        const val = bytes / Math.pow(1024, i);
        return val.toFixed(i > 0 ? 1 : 0) + ' ' + units[i];
    }}

    function formatDuration(secs) {{
        if (secs == null) return '-';
        const d = Math.floor(secs / 86400);
        const h = Math.floor((secs % 86400) / 3600);
        const m = Math.floor((secs % 3600) / 60);
        const s = Math.floor(secs % 60);
        if (d > 0) return `${{d}}d ${{h}}h ${{m}}m`;
        if (h > 0) return `${{h}}h ${{m}}m ${{s}}s`;
        if (m > 0) return `${{m}}m ${{s}}s`;
        return `${{s}}s`;
    }}

    function timeAgo(ts) {{
        if (!ts) return '-';
        const diff = (Date.now() - new Date(ts).getTime()) / 1000;
        if (diff < 60) return Math.floor(diff) + 's ago';
        if (diff < 3600) return Math.floor(diff / 60) + 'm ago';
        if (diff < 86400) return Math.floor(diff / 3600) + 'h ago';
        return Math.floor(diff / 86400) + 'd ago';
    }}

    async function apiPost(url) {{
        try {{
            const resp = await fetch(url, {{ method: 'POST' }});
            await resp.json();
            refresh();
        }} catch (e) {{ console.error(e); }}
    }}

    function drawMemChart(samples) {{
        const canvas = document.getElementById('mem-chart');
        if (!canvas) return;
        const ctx = canvas.getContext('2d');
        const dpr = window.devicePixelRatio || 1;
        const rect = canvas.parentElement.getBoundingClientRect();
        canvas.width = rect.width * dpr;
        canvas.height = rect.height * dpr;
        ctx.scale(dpr, dpr);
        const W = rect.width, H = rect.height;

        ctx.clearRect(0, 0, W, H);
        if (samples.length < 2) {{
            ctx.fillStyle = '#666';
            ctx.font = '12px system-ui';
            ctx.fillText('Collecting data...', W / 2 - 50, H / 2);
            return;
        }}

        const maxRss = Math.max(...samples.map(s => s.rss_bytes)) * 1.1 || 1;

        ctx.strokeStyle = '#1a1d32';
        ctx.lineWidth = 1;
        for (let i = 0; i <= 4; i++) {{
            const y = H - (i / 4) * (H - 20);
            ctx.beginPath(); ctx.moveTo(40, y); ctx.lineTo(W, y); ctx.stroke();
            ctx.fillStyle = '#555';
            ctx.font = '10px system-ui';
            ctx.fillText(formatBytes(maxRss * i / 4), 0, y + 3);
        }}

        if (samples.length > 1) {{
            const first = new Date(samples[0].timestamp);
            const last = new Date(samples[samples.length - 1].timestamp);
            ctx.fillStyle = '#555';
            ctx.font = '10px system-ui';
            ctx.fillText(first.toLocaleTimeString(), 40, H - 2);
            ctx.fillText(last.toLocaleTimeString(), W - 60, H - 2);
        }}

        ctx.strokeStyle = '#50fa7b';
        ctx.lineWidth = 2;
        ctx.beginPath();
        samples.forEach((s, i) => {{
            const x = 40 + (i / (samples.length - 1)) * (W - 44);
            const y = (H - 20) - (s.rss_bytes / maxRss) * (H - 40) + 10;
            if (i === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
        }});
        ctx.stroke();

        ctx.strokeStyle = 'rgba(139,233,253,0.3)';
        ctx.lineWidth = 1;
        const maxVms = Math.max(...samples.map(s => s.vms_bytes)) * 1.1 || 1;
        ctx.beginPath();
        samples.forEach((s, i) => {{
            const x = 40 + (i / (samples.length - 1)) * (W - 44);
            const y = (H - 20) - (s.vms_bytes / maxVms) * (H - 40) + 10;
            if (i === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
        }});
        ctx.stroke();

        ctx.fillStyle = '#50fa7b'; ctx.fillRect(W - 100, 6, 10, 3);
        ctx.fillStyle = '#888'; ctx.font = '10px system-ui'; ctx.fillText('RSS', W - 86, 12);
        ctx.fillStyle = 'rgba(139,233,253,0.5)'; ctx.fillRect(W - 55, 6, 10, 3);
        ctx.fillStyle = '#888'; ctx.fillText('VMS', W - 41, 12);
    }}

    async function refreshMemory() {{
        try {{
            const [statsResp, histResp] = await Promise.all([
                fetch('/api/v1/stats'),
                fetch('/api/v1/memory/history'),
            ]);
            const stats = await statsResp.json();
            const history = await histResp.json();

            const memDiv = document.getElementById('mem-current');
            if (stats.memory) {{
                memDiv.innerHTML = `RSS: <span style="color:#50fa7b;font-weight:600">${{formatBytes(stats.memory.rss_bytes)}}</span> &nbsp; VMS: <span style="color:#8be9fd">${{formatBytes(stats.memory.vms_bytes)}}</span>`;
            }} else {{
                memDiv.innerHTML = '<span style="color:#666">Memory info not available (non-Linux)</span>';
            }}

            drawMemChart(history || []);
        }} catch (_) {{}}
    }}

    async function refreshMounts() {{
        try {{
            const resp = await fetch('/api/v1/mounts');
            const mounts = await resp.json();
            const div = document.getElementById('mounts');
            if (!mounts || mounts.length === 0) {{
                div.innerHTML = '<span style="color:#666;font-size:13px">No mount info available (non-Linux or no block mounts)</span>';
                return;
            }}
            div.innerHTML = '<table style="width:100%"><thead><tr>' +
                '<th>Mount</th><th>Device</th><th>Type</th><th>Used</th><th>Total</th><th>Free</th><th style="width:30%">Usage</th>' +
                '</tr></thead><tbody>' +
                mounts.map(m => {{
                    const pct = Math.round(m.use_percent || 0);
                    const color = pct > 90 ? '#e94560' : pct > 75 ? '#f0a030' : '#50fa7b';
                    return '<tr>' +
                        '<td class="mono">' + escapeHtml(m.mount_point) + '</td>' +
                        '<td class="mono" style="color:#666;font-size:12px">' + escapeHtml(m.device) + '</td>' +
                        '<td style="color:#555;font-size:12px">' + escapeHtml(m.fs_type) + '</td>' +
                        '<td>' + formatBytes(m.used_bytes) + '</td>' +
                        '<td>' + formatBytes(m.total_bytes) + '</td>' +
                        '<td style="color:#50fa7b">' + formatBytes(m.avail_bytes) + '</td>' +
                        '<td><div class="usage-bar">' +
                            '<div class="usage-bar-fill" style="width:' + pct + '%;background:' + color + '"></div>' +
                            '<div class="usage-bar-text">' + pct + '%</div>' +
                        '</div></td></tr>';
                }}).join('') +
                '</tbody></table>';
        }} catch (_) {{}}
    }}

    async function refresh() {{
        try {{
            const [procResp, statusResp] = await Promise.all([
                fetch('/api/v1/processes'),
                fetch('/api/v1/status'),
            ]);
            const procs = await procResp.json();
            const status = await statusResp.json();

            const total = procs.length;
            const running = procs.filter(p => (p.state || '').toLowerCase() === 'running').length;
            const crashes = procs.reduce((sum, p) => sum + (p.crashes || 0), 0);
            const restarts = procs.reduce((sum, p) => sum + (p.restarts || 0), 0);
            const mem = status.stats && status.stats.memory;

            document.getElementById('stats').innerHTML = `
                <div class="stat-card"><div class="label">Processes</div><div class="value cyan">${{total}}</div></div>
                <div class="stat-card"><div class="label">Running</div><div class="value green">${{running}}</div></div>
                <div class="stat-card"><div class="label">Crashes</div><div class="value ${{crashes > 0 ? 'red' : 'green'}}">${{crashes}}</div></div>
                <div class="stat-card"><div class="label">Total Restarts</div><div class="value yellow">${{restarts}}</div></div>
                <div class="stat-card"><div class="label">Memory (RSS)</div><div class="value green">${{mem ? formatBytes(mem.rss_bytes) : '-'}}</div></div>
                <div class="stat-card"><div class="label">Container</div><div class="value ${{status.container_failed ? 'red' : 'green'}}">${{status.container_failed ? 'FAILED' : 'HEALTHY'}}</div></div>
            `;

            const tbody = document.getElementById('procs');
            tbody.innerHTML = procs.map(p => {{
                const rts = p.restart_timestamps || [];
                const lastRestart = rts.length > 0 ? timeAgo(rts[rts.length - 1]) : '-';
                return `<tr>
                    <td class="mono">${{escapeHtml(p.name)}}</td>
                    <td>${{stateBadge(p.state)}}</td>
                    <td class="mono">${{p.pid || '-'}}</td>
                    <td class="mono">${{p.exit_code != null ? p.exit_code : '-'}}</td>
                    <td style="color:${{(p.crashes || 0) > 0 ? '#e94560' : '#50fa7b'}}">${{p.crashes || 0}}</td>
                    <td>${{p.restarts || 0}}</td>
                    <td style="font-size:12px;color:#888">${{lastRestart}}</td>
                    <td>${{formatDuration(p.uptime_secs)}}</td>
                    <td class="process-actions">
                        <button class="btn-green" onclick="apiPost('/api/v1/processes/${{encodeURIComponent(p.name)}}/start')">Start</button>
                        <button class="btn-red" onclick="apiPost('/api/v1/processes/${{encodeURIComponent(p.name)}}/stop')">Stop</button>
                        <button class="btn-yellow" onclick="apiPost('/api/v1/processes/${{encodeURIComponent(p.name)}}/restart')">Restart</button>
                    </td>
                </tr>`;
            }}).join('');

            const allRestarts = [];
            procs.forEach(p => {{
                (p.restart_timestamps || []).forEach(ts => {{
                    allRestarts.push({{ process: p.name, timestamp: ts }});
                }});
            }});
            allRestarts.sort((a, b) => new Date(b.timestamp) - new Date(a.timestamp));
            if (allRestarts.length > 0) {{
                document.getElementById('restart-section').style.display = '';
                const histDiv = document.getElementById('restart-history');
                histDiv.innerHTML = '<div class="restart-list">' + allRestarts.slice(0, 50).map(r => {{
                    const dt = new Date(r.timestamp);
                    const logsUrl = '/ui/logs?process=' + encodeURIComponent(r.process);
                    return `<div><a href="${{logsUrl}}" style="color:#8be9fd" title="View logs">${{escapeHtml(r.process)}}</a> <span style="color:#555">${{dt.toLocaleString()}}</span> <span style="color:#666">(${{timeAgo(r.timestamp)}})</span></div>`;
                }}).join('') + '</div>';
            }}

            const cronResp = await fetch('/api/v1/cron');
            const crons = await cronResp.json();
            if (crons.length > 0) {{
                document.getElementById('cron-section').style.display = '';
                document.getElementById('crons').innerHTML = crons.map(c => `<tr>
                    <td class="mono">${{escapeHtml(c.name)}}</td>
                    <td class="mono">${{escapeHtml(c.schedule)}}</td>
                    <td>${{c.run_count}}</td>
                    <td>${{c.fail_count > 0 ? '<span class="badge badge-red">' + c.fail_count + '</span>' : '0'}}</td>
                    <td>${{c.next_run || '-'}}</td>
                </tr>`).join('');
            }}

            try {{
                const updResp = await fetch('/api/v1/updates');
                const updates = await updResp.json();
                if (Array.isArray(updates) && updates.length > 0) {{
                    document.getElementById('updates-section').style.display = '';
                    document.getElementById('updates').innerHTML = updates.map(u => `<tr>
                        <td class="mono">${{escapeHtml(u.process || u.name || '')}}</td>
                        <td class="mono" style="font-size:12px">${{escapeHtml(u.image || '')}}</td>
                        <td class="mono" style="font-size:11px;color:#666">${{escapeHtml((u.current_digest || '').substring(0, 16))}}</td>
                        <td>${{stateBadge(u.status || 'idle')}}</td>
                        <td><button onclick="apiPost('/api/v1/updates/${{encodeURIComponent(u.process || u.name)}}/trigger')">Update Now</button></td>
                    </tr>`).join('');
                }}
            }} catch (_) {{}}

        }} catch (e) {{ console.error('refresh error:', e); }}
    }}

    refresh();
    refreshMemory();
    refreshMounts();
    setInterval(refresh, 3000);
    setInterval(refreshMemory, 5000);
    setInterval(refreshMounts, 15000);
    </script>
</body>
</html>"#,
        css = nav_css(),
        nav = nav_html("Dashboard", name),
        ansi_js = ansi_js(),
    )
}

// --- Terminal page ---

fn build_terminal(name: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html>
<head>
    <title>stormd — Terminal</title>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <style>
{css}
    .toolbar {{ padding: 12px 20px; display: flex; gap: 12px; align-items: center; border-bottom: 1px solid #2a2d45; }}
    </style>
</head>
<body>
    {nav}
    <div class="toolbar controls">
        <label style="font-size:13px;color:#888">Process:</label>
        <select id="process" onchange="connect()"></select>
        <span id="conn-status" class="badge badge-gray">disconnected</span>
    </div>
    <div class="content" style="padding-top:8px">
        <div id="terminal" class="term-output"></div>
    </div>
    <script>
{ansi_js}

    let ws = null;
    const terminal = document.getElementById('terminal');
    const processSelect = document.getElementById('process');
    const connStatus = document.getElementById('conn-status');

    async function loadProcesses() {{
        try {{
            const resp = await fetch('/api/v1/processes');
            const procs = await resp.json();
            const prev = processSelect.value;
            processSelect.innerHTML = '';
            procs.forEach(p => {{
                const opt = document.createElement('option');
                opt.value = p.name;
                opt.textContent = `${{p.name}} (${{(p.state || '').toLowerCase()}})`;
                if (p.name === prev) opt.selected = true;
                processSelect.appendChild(opt);
            }});
            if (!prev && procs.length > 0) connect();
        }} catch (_) {{}}
    }}

    function connect() {{
        if (ws) ws.close();
        terminal.innerHTML = '';
        const process = processSelect.value;
        if (!process) return;

        connStatus.className = 'badge badge-yellow';
        connStatus.textContent = 'connecting';

        const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
        ws = new WebSocket(`${{proto}}//${{location.host}}/ws/console/${{encodeURIComponent(process)}}`);

        ws.onopen = () => {{
            connStatus.className = 'badge badge-green';
            connStatus.textContent = 'connected';
        }};

        ws.onmessage = (e) => {{
            const msg = JSON.parse(e.data);
            if (msg.type === 'snapshot') {{
                terminal.innerHTML = '<div style="color:#666">--- terminal snapshot ---</div>' +
                    ansiToHtml(msg.data.contents || '') +
                    '<div style="color:#666">--- live output ---</div>\n';
            }} else if (msg.type === 'entry') {{
                const cls = msg.data.stream || 'stdout';
                const ts = new Date(msg.data.timestamp).toLocaleTimeString();
                const color = cls === 'stderr' ? 'color:#e94560' : '';
                const line = `<div class="log-entry" style="${{color}}"><span style="color:#666">${{escapeHtml(ts)}}</span> <span style="color:#555">[${{escapeHtml(cls)}}]</span> ${{ansiToHtml(msg.data.line || '')}}</div>`;
                terminal.insertAdjacentHTML('beforeend', line);
                terminal.scrollTop = terminal.scrollHeight;
            }} else if (msg.type === 'lagged') {{
                terminal.insertAdjacentHTML('beforeend', `<div style="color:#f0a030">--- skipped ${{msg.skipped}} entries ---</div>`);
            }}
        }};

        ws.onclose = () => {{
            connStatus.className = 'badge badge-red';
            connStatus.textContent = 'disconnected';
            terminal.insertAdjacentHTML('beforeend', '<div style="color:#666">--- disconnected ---</div>');
        }};
    }}

    loadProcesses();
    setInterval(loadProcesses, 10000);
    </script>
</body>
</html>"#,
        css = nav_css(),
        nav = nav_html("Terminal", name),
        ansi_js = ansi_js(),
    )
}

// --- Logs page ---

fn build_logs(name: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html>
<head>
    <title>stormd — Logs</title>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <style>
{css}
    .toolbar {{ padding: 12px 20px; display: flex; gap: 12px; align-items: center; border-bottom: 1px solid #2a2d45; flex-wrap: wrap; }}
    .log-count {{ font-size: 12px; color: #666; margin-left: auto; }}
    label.cb {{ display: flex; align-items: center; gap: 4px; font-size: 13px; color: #888; cursor: pointer; }}
    label.cb input {{ accent-color: #50fa7b; }}
    .run-tag {{ font-size: 10px; padding: 1px 6px; border-radius: 8px; margin-left: 4px; }}
    .run-tag.failed {{ background: #4a1a2a; color: #e94560; }}
    .run-tag.exited {{ background: #1a3a2a; color: #50fa7b; }}
    .run-tag.current {{ background: #1a3a4a; color: #8be9fd; }}
    .run-info {{ font-size: 11px; color: #666; padding: 8px 20px; border-bottom: 1px solid #1a1d32; }}
    </style>
</head>
<body>
    {nav}
    <div class="toolbar controls">
        <label style="font-size:13px;color:#888">Process:</label>
        <select id="process"><option value="">All</option></select>
        <label style="font-size:13px;color:#888">Run:</label>
        <select id="run"><option value="">Live</option></select>
        <label style="font-size:13px;color:#888">Severity:</label>
        <select id="severity">
            <option value="">All</option>
            <option value="emergency">Emergency</option>
            <option value="critical">Critical</option>
            <option value="error">Error</option>
            <option value="warning">Warning</option>
            <option value="info" selected>Info+</option>
            <option value="debug">Debug</option>
        </select>
        <input id="search" type="search" placeholder="Search..." style="width:200px">
        <label class="cb"><input type="checkbox" id="follow" checked> Follow</label>
        <button onclick="clearLogs()">Clear</button>
        <span class="log-count" id="count">0 entries</span>
    </div>
    <div id="run-info" class="run-info" style="display:none"></div>
    <div class="content" style="padding-top:8px">
        <div id="logs" class="term-output"></div>
    </div>
    <script>
{ansi_js}

    let ws = null;
    let entryCount = 0;
    let currentMode = 'live'; // 'live' or 'stored'
    const logsDiv = document.getElementById('logs');
    const countEl = document.getElementById('count');
    const searchInput = document.getElementById('search');
    const runSelect = document.getElementById('run');
    const runInfoDiv = document.getElementById('run-info');

    function clearLogs() {{
        logsDiv.innerHTML = '';
        entryCount = 0;
        countEl.textContent = '0 entries';
    }}

    function formatBytes(bytes) {{
        if (bytes == null || bytes === 0) return '0 B';
        const units = ['B', 'KiB', 'MiB', 'GiB'];
        const i = Math.floor(Math.log(bytes) / Math.log(1024));
        return (bytes / Math.pow(1024, i)).toFixed(i > 0 ? 1 : 0) + ' ' + units[i];
    }}

    async function loadProcesses() {{
        try {{
            const resp = await fetch('/api/v1/processes');
            const procs = await resp.json();
            const sel = document.getElementById('process');
            const current = sel.value;
            sel.innerHTML = '<option value="">All</option>';
            procs.forEach(p => {{
                const opt = document.createElement('option');
                opt.value = p.name;
                opt.textContent = p.name;
                if (p.name === current) opt.selected = true;
                sel.appendChild(opt);
            }});
        }} catch (_) {{}}
    }}

    async function loadRuns() {{
        const process = document.getElementById('process').value;
        const currentRun = runSelect.value;
        runSelect.innerHTML = '<option value="">Live (streaming)</option>';

        if (!process) {{
            runSelect.disabled = true;
            return;
        }}

        runSelect.disabled = false;

        try {{
            const resp = await fetch('/api/v1/logs/' + encodeURIComponent(process) + '/runs');
            const data = await resp.json();
            const runs = data.runs || [];
            const currentRunId = data.current_run_id;

            if (currentRunId) {{
                const opt = document.createElement('option');
                opt.value = currentRunId;
                opt.textContent = formatRunId(currentRunId) + ' (current)';
                opt.dataset.current = 'true';
                runSelect.appendChild(opt);
            }}

            runs.forEach(r => {{
                if (r.run_id === currentRunId) return;
                const opt = document.createElement('option');
                opt.value = r.run_id;
                const size = formatBytes(r.size_bytes);
                opt.textContent = formatRunId(r.run_id) + ' — ' + r.date + ' (' + size + ')';
                runSelect.appendChild(opt);
            }});

            // Also load local archived files
            try {{
                const filesResp = await fetch('/api/v1/logs/files');
                const files = await filesResp.json();
                files.forEach(f => {{
                    const name = f.name || '';
                    // Match pattern: process.RUNID.(failed or exited).log
                    const m = name.match(new RegExp('^' + escapeRegex(process) + '\\.(\\d{{8}}T\\d{{6}})\\.(failed|exited)\\.log$'));
                    if (m) {{
                        const runId = m[1];
                        const tag = m[2];
                        // Skip if already listed from MinIO
                        const exists = Array.from(runSelect.options).some(o => o.value === runId);
                        if (!exists) {{
                            const opt = document.createElement('option');
                            opt.value = 'local:' + name;
                            opt.textContent = formatRunId(runId) + ' [' + tag + '] (local, ' + formatBytes(f.size_bytes) + ')';
                            if (tag === 'failed') opt.style.color = '#e94560';
                            runSelect.appendChild(opt);
                        }}
                    }}
                }});
            }} catch (_) {{}}

            if (currentRun) {{
                for (const opt of runSelect.options) {{
                    if (opt.value === currentRun) {{ opt.selected = true; break; }}
                }}
            }}
        }} catch (_) {{}}
    }}

    function escapeRegex(s) {{
        return s.replace(/[.*+?^$|[\]\\]/g, '\\$&').replace(/\{{/g, '\\{{').replace(/\}}/g, '\\}}');
    }}

    function formatRunId(rid) {{
        // 20260318T000854 -> 2026-03-18 00:08:54
        if (rid && rid.length === 15) {{
            return rid.substring(0,4) + '-' + rid.substring(4,6) + '-' + rid.substring(6,8) +
                   ' ' + rid.substring(9,11) + ':' + rid.substring(11,13) + ':' + rid.substring(13,15);
        }}
        return rid || '';
    }}

    function switchMode() {{
        const runVal = runSelect.value;
        clearLogs();

        if (!runVal) {{
            // Live mode
            currentMode = 'live';
            runInfoDiv.style.display = 'none';
            document.getElementById('follow').parentElement.style.display = '';
            connectWs();
        }} else if (runVal.startsWith('local:')) {{
            // Local archived file
            currentMode = 'stored';
            if (ws) {{ ws.close(); ws = null; }}
            document.getElementById('follow').parentElement.style.display = 'none';
            loadLocalFile(runVal.substring(6));
        }} else {{
            // Stored run from MinIO
            currentMode = 'stored';
            if (ws) {{ ws.close(); ws = null; }}
            document.getElementById('follow').parentElement.style.display = 'none';
            loadStoredRun(runVal);
        }}
    }}

    async function loadStoredRun(runId) {{
        const process = document.getElementById('process').value;
        runInfoDiv.style.display = '';
        runInfoDiv.innerHTML = 'Loading run <span style="color:#8be9fd">' + escapeHtml(formatRunId(runId)) + '</span> ...';

        try {{
            let url = '/api/v1/logs/stored?process=' + encodeURIComponent(process) + '&run_id=' + encodeURIComponent(runId);
            const search = searchInput.value;
            if (search) url += '&search=' + encodeURIComponent(search);

            const resp = await fetch(url);
            const entries = await resp.json();

            clearLogs();
            entries.forEach(e => appendLog(e));

            const failed = entries.some(e => (e.line || '').includes('--- process exited ---'));
            runInfoDiv.innerHTML = 'Run <span style="color:#8be9fd">' + escapeHtml(formatRunId(runId)) + '</span> — ' +
                entries.length + ' entries' +
                (entries.length > 0 ? ' — ' + new Date(entries[0].timestamp).toLocaleString() + ' to ' + new Date(entries[entries.length-1].timestamp).toLocaleString() : '');
        }} catch (e) {{
            runInfoDiv.innerHTML = '<span style="color:#e94560">Failed to load run: ' + escapeHtml(e.message) + '</span>';
        }}
    }}

    async function loadLocalFile(filename) {{
        runInfoDiv.style.display = '';
        runInfoDiv.innerHTML = 'Loading local archive <span style="color:#f1fa8c">' + escapeHtml(filename) + '</span> ...';

        try {{
            const resp = await fetch('/api/v1/logs/files/' + encodeURIComponent(filename) + '?tail=10000');
            const data = await resp.json();
            clearLogs();

            (data.lines || []).forEach(line => {{
                const html = '<div class="log-entry">' + ansiToHtml(line) + '</div>';
                logsDiv.insertAdjacentHTML('beforeend', html);
                entryCount++;
            }});
            countEl.textContent = entryCount + ' lines';

            const isFailed = filename.includes('.failed.');
            const tag = isFailed ? '<span class="run-tag failed">FAILED</span>' : '<span class="run-tag exited">EXITED</span>';
            runInfoDiv.innerHTML = 'Local archive: <span style="color:#f1fa8c">' + escapeHtml(filename) + '</span> ' + tag + ' — ' + entryCount + ' lines';
        }} catch (e) {{
            runInfoDiv.innerHTML = '<span style="color:#e94560">Failed to load: ' + escapeHtml(e.message) + '</span>';
        }}
    }}

    async function loadRecentLines() {{
        const process = document.getElementById('process').value;
        if (!process) return;

        try {{
            const resp = await fetch('/api/v1/logs/' + encodeURIComponent(process) + '?tail=100');
            const data = await resp.json();
            if (data.lines && data.lines.length > 0) {{
                data.lines.forEach(line => {{
                    const html = '<div class="log-entry" style="color:#555">' + ansiToHtml(line) + '</div>';
                    logsDiv.insertAdjacentHTML('beforeend', html);
                    entryCount++;
                }});
                logsDiv.insertAdjacentHTML('beforeend', '<div style="color:#666;border-top:1px solid #2a2d45;padding:4px 0;margin:4px 0;font-size:11px">--- recent history above, live stream below ---</div>');
                countEl.textContent = entryCount + ' lines';
                logsDiv.scrollTop = logsDiv.scrollHeight;
            }}
        }} catch (_) {{}}
    }}

    function connectWs() {{
        if (ws) ws.close();
        runInfoDiv.style.display = 'none';
        const process = document.getElementById('process').value;
        const severity = document.getElementById('severity').value;

        let url = (location.protocol === 'https:' ? 'wss:' : 'ws:') + '//' + location.host + '/ws/logs?';
        if (process) url += 'process=' + encodeURIComponent(process) + '&';
        if (severity) url += 'severity=' + encodeURIComponent(severity) + '&';

        ws = new WebSocket(url);
        ws.onmessage = (e) => {{
            const entry = JSON.parse(e.data);
            appendLog(entry);
        }};
    }}

    function sevColor(sev) {{
        const s = (sev || '').toLowerCase();
        if (s === 'error' || s === 'critical' || s === 'emergency') return 'color:#e94560';
        if (s === 'warning') return 'color:#f0a030';
        if (s === 'debug') return 'color:#555';
        return '';
    }}

    function appendLog(entry) {{
        const search = searchInput.value;
        if (search && !(entry.line || '').includes(search)) return;

        const ts = new Date(entry.timestamp).toLocaleTimeString();
        const sc = sevColor(entry.severity);
        const html = '<div class="log-entry" style="' + sc + '"><span style="color:#666">' + escapeHtml(ts) + '</span> <span style="color:#8be9fd">' + escapeHtml(entry.process || '') + '</span> <span style="color:#555">[' + escapeHtml(entry.stream || '') + ']</span> ' + ansiToHtml(entry.line || '') + '</div>';
        logsDiv.insertAdjacentHTML('beforeend', html);

        entryCount++;
        countEl.textContent = entryCount + ' entries';

        if (document.getElementById('follow').checked) {{
            logsDiv.scrollTop = logsDiv.scrollHeight;
        }}

        while (logsDiv.children.length > 5000) {{
            logsDiv.removeChild(logsDiv.firstChild);
            entryCount--;
        }}
    }}

    // Check for ?process= query param (linked from dashboard restart history)
    const urlParams = new URLSearchParams(window.location.search);
    const preselect = urlParams.get('process');

    loadProcesses().then(() => {{
        if (preselect) {{
            const sel = document.getElementById('process');
            for (const opt of sel.options) {{
                if (opt.value === preselect) {{ opt.selected = true; break; }}
            }}
            loadRuns();
        }}
        connectWs();
        loadRecentLines();
    }});

    document.getElementById('process').onchange = () => {{
        loadRuns();
        runSelect.value = '';
        clearLogs();
        connectWs();
        loadRecentLines();
    }};
    runSelect.onchange = () => {{ switchMode(); }};
    document.getElementById('severity').onchange = () => {{
        if (currentMode === 'live') {{ clearLogs(); connectWs(); }}
    }};
    searchInput.addEventListener('keyup', (e) => {{
        if (e.key === 'Enter') {{
            if (currentMode === 'live') {{ clearLogs(); connectWs(); }}
            else {{ switchMode(); }}
        }}
    }});
    </script>
</body>
</html>"#,
        css = nav_css(),
        nav = nav_html("Logs", name),
        ansi_js = ansi_js(),
    )
}
