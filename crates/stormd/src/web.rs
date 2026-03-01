use axum::response::Html;

/// Serve the web terminal page with xterm.js.
pub async fn terminal_page() -> Html<&'static str> {
    Html(TERMINAL_HTML)
}

/// Serve the log viewer page.
pub async fn logs_page() -> Html<&'static str> {
    Html(LOGS_HTML)
}

const TERMINAL_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <title>stormd — Terminal</title>
    <meta charset="utf-8">
    <style>
        * { margin: 0; padding: 0; box-sizing: border-box; }
        body { background: #1a1a2e; color: #eee; font-family: monospace; }
        #header { background: #16213e; padding: 8px 16px; display: flex; align-items: center; gap: 16px; }
        #header h1 { font-size: 16px; color: #0f3460; }
        #header h1 { color: #e94560; }
        select { background: #0f3460; color: #eee; border: 1px solid #333; padding: 4px 8px; border-radius: 4px; }
        #terminal { width: 100%; height: calc(100vh - 44px); background: #0a0a1a; padding: 8px; overflow-y: auto; white-space: pre-wrap; font-size: 14px; line-height: 1.4; }
        .entry { }
        .stdout { color: #ccc; }
        .stderr { color: #e94560; }
        .syslog { color: #0f9; }
        .meta { color: #666; }
    </style>
</head>
<body>
    <div id="header">
        <h1>stormd</h1>
        <label>Process: <select id="process" onchange="connect()"></select></label>
    </div>
    <div id="terminal"></div>
    <script>
        let ws = null;
        const terminal = document.getElementById('terminal');
        const processSelect = document.getElementById('process');

        async function loadProcesses() {
            const resp = await fetch('/api/v1/processes');
            const procs = await resp.json();
            processSelect.innerHTML = '';
            procs.forEach(p => {
                const opt = document.createElement('option');
                opt.value = p.name;
                opt.textContent = `${p.name} (${p.state})`;
                processSelect.appendChild(opt);
            });
            if (procs.length > 0) connect();
        }

        function connect() {
            if (ws) ws.close();
            terminal.innerHTML = '';
            const process = processSelect.value;
            if (!process) return;

            const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
            ws = new WebSocket(`${proto}//${location.host}/ws/console/${process}`);

            ws.onmessage = (e) => {
                const msg = JSON.parse(e.data);
                if (msg.type === 'snapshot') {
                    terminal.innerHTML = `<span class="meta">--- Terminal snapshot ---</span>\n${escapeHtml(msg.data.contents)}\n<span class="meta">--- Live output ---</span>\n`;
                } else if (msg.type === 'entry') {
                    const cls = msg.data.stream || 'stdout';
                    const ts = new Date(msg.data.timestamp).toLocaleTimeString();
                    const line = document.createElement('div');
                    line.className = `entry ${cls}`;
                    line.textContent = `${ts} [${cls}] ${msg.data.line}`;
                    terminal.appendChild(line);
                    terminal.scrollTop = terminal.scrollHeight;
                }
            };

            ws.onclose = () => {
                const div = document.createElement('div');
                div.className = 'meta';
                div.textContent = '--- disconnected ---';
                terminal.appendChild(div);
            };
        }

        function escapeHtml(s) {
            return s.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
        }

        loadProcesses();
        setInterval(loadProcesses, 10000);
    </script>
</body>
</html>"#;

const LOGS_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <title>stormd — Logs</title>
    <meta charset="utf-8">
    <style>
        * { margin: 0; padding: 0; box-sizing: border-box; }
        body { background: #1a1a2e; color: #eee; font-family: monospace; }
        #header { background: #16213e; padding: 8px 16px; display: flex; align-items: center; gap: 16px; }
        #header h1 { color: #e94560; font-size: 16px; }
        select, input { background: #0f3460; color: #eee; border: 1px solid #333; padding: 4px 8px; border-radius: 4px; }
        input { width: 200px; }
        #logs { width: 100%; height: calc(100vh - 44px); background: #0a0a1a; padding: 8px; overflow-y: auto; font-size: 13px; line-height: 1.3; }
        .log-line { white-space: pre-wrap; }
        .sev-error, .sev-critical, .sev-emergency { color: #e94560; }
        .sev-warning { color: #f0a030; }
        .sev-debug { color: #666; }
        .sev-info, .sev-notice { color: #ccc; }
        .process { color: #0af; }
        .timestamp { color: #666; }
    </style>
</head>
<body>
    <div id="header">
        <h1>stormd logs</h1>
        <label>Process: <select id="process"><option value="">All</option></select></label>
        <label>Severity: <select id="severity">
            <option value="">All</option>
            <option value="emergency">Emergency</option>
            <option value="error">Error</option>
            <option value="warning">Warning</option>
            <option value="info" selected>Info+</option>
            <option value="debug">Debug</option>
        </select></label>
        <input id="search" placeholder="Search..." onkeyup="if(event.key==='Enter')loadLogs()">
        <label><input type="checkbox" id="follow" checked> Follow</label>
    </div>
    <div id="logs"></div>
    <script>
        let ws = null;
        const logsDiv = document.getElementById('logs');

        async function loadProcesses() {
            const resp = await fetch('/api/v1/processes');
            const procs = await resp.json();
            const sel = document.getElementById('process');
            const current = sel.value;
            sel.innerHTML = '<option value="">All</option>';
            procs.forEach(p => {
                const opt = document.createElement('option');
                opt.value = p.name;
                opt.textContent = p.name;
                if (p.name === current) opt.selected = true;
                sel.appendChild(opt);
            });
        }

        function connectWs() {
            if (ws) ws.close();
            const process = document.getElementById('process').value;
            const severity = document.getElementById('severity').value;

            let url = `${location.protocol === 'https:' ? 'wss:' : 'ws:'}//${location.host}/ws/logs?`;
            if (process) url += `process=${process}&`;
            if (severity) url += `severity=${severity}&`;

            ws = new WebSocket(url);
            ws.onmessage = (e) => {
                const entry = JSON.parse(e.data);
                appendLog(entry);
            };
        }

        function appendLog(entry) {
            const search = document.getElementById('search').value;
            if (search && !entry.line.includes(search)) return;

            const div = document.createElement('div');
            div.className = `log-line sev-${entry.severity}`;
            const ts = new Date(entry.timestamp).toLocaleTimeString();
            div.innerHTML = `<span class="timestamp">${ts}</span> <span class="process">${entry.process}</span> [${entry.stream}] ${escapeHtml(entry.line)}`;
            logsDiv.appendChild(div);

            if (document.getElementById('follow').checked) {
                logsDiv.scrollTop = logsDiv.scrollHeight;
            }

            // Limit DOM size
            while (logsDiv.children.length > 5000) {
                logsDiv.removeChild(logsDiv.firstChild);
            }
        }

        function escapeHtml(s) {
            return s.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
        }

        loadProcesses();
        connectWs();

        document.getElementById('process').onchange = connectWs;
        document.getElementById('severity').onchange = connectWs;
    </script>
</body>
</html>"#;
