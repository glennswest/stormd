# Plugin UI

A supervised process that serves its own web UI can appear as a tab in
stormd's web console, and can put its own numbers on its dashboard card,
without any change to stormd. Written from `api.rs` (`proxy_plugin`),
`components.rs` and `main.rs` at v0.7.0.

## Configuration

```toml
[[process]]
name = "myapp"
command = "/app/myapp"
args = ["--port", "3000"]

[process.ui]
label = "My App"                                 # the nav tab
proxy = "http://127.0.0.1:3000"                  # where its UI is served
# host = "myapp.example.lo"                      # optional, see below
# summary = "http://127.0.0.1:3000/api/summary"  # optional, see below
```

`GET /api/v1/plugins` lists what is registered.

## The tab and the proxy

The tab opens `/ui/#/ext/myapp`, which shows stormd's nav and an iframe of
`/ui/proxy/myapp/`. Everything under that prefix is proxied:

```
/ui/proxy/myapp/        -> http://127.0.0.1:3000/
/ui/proxy/myapp/foo?x=1 -> http://127.0.0.1:3000/foo?x=1
```

What the proxy does, exactly:

- Same origin as stormd, so the plugin need not be reachable from the browser
  and needs no CORS.
- Methods: GET, POST, PUT, DELETE, PATCH, HEAD (anything else is sent as GET).
- Request: forwards the `Content-Type` header and, for non-GET/HEAD, the body
  **as text** — binary uploads do not survive. No other request header is
  forwarded: not `Cookie`, not `Authorization`.
- Response: the status and `Content-Type` only — no `Set-Cookie`, no
  `Location`, no caching headers. Redirects are followed by the proxy itself.
- No WebSocket upgrade.
- A plugin that cannot be reached answers 500 with `{"error": "proxy: ..."}`.
- With stormd auth on, `/ui/proxy/*` requires a session (the browser's
  session cookie rides along with the iframe's requests).

So a plugin UI should use relative URLs (it is mounted under a prefix), keep
its state in the page or its own API rather than cookies, and use plain
`fetch` for anything live.

## Host-based routing

`host = "myapp.example.lo"` adds that name to stormd's host map: a request for
`/` on stormd's port with `Host: myapp.example.lo` is redirected to
`/ui/proxy/myapp/`. Only `/` is redirected — the rest of the paths are
stormd's. `[api.hosts]` does the same for arbitrary targets.

## Summary: the plugin's own card

With `summary` set, every build of the component feed (`/api/v1/components`,
and `/ws/components` every 2 s) GETs that URL with a 400 ms timeout,
concurrently with other plugins, and merges the JSON into the plugin's card:

```json
{
  "health": "ok",
  "detail": "serving 42 clients",
  "metrics": [
    { "label": "clients", "value": "42", "tone": "accent" },
    { "label": "queue", "value": "0", "unit": "jobs", "tone": "muted" }
  ]
}
```

Every field is optional. `health` (`ok`, `warn`, `error`, `idle`, `unknown`)
and `detail` replace stormd's process-level view of the card; `metrics` are
appended after stormd's own. `tone` is a rendering hint: `ok`, `warn`,
`error`, `muted`, `accent`. A slow, failing or malformed endpoint costs the
card only the extra detail. The types are the
[stormview](https://github.com/glennswest/stormview) crate's `Health` and
`Metric`.

## Looking like the console

The iframe does not inherit stormd's styles. To match, use the design tokens
from stormview's `web/themes.css` (`--bg`, `--panel`, `--border`, `--text`,
`--text-dim`, `--ok`, `--warn`, `--error`, `--accent`, the `--ansi-*`
palette, …) — the same CSS custom properties every theme overrides — or
consume the stormview npm package directly, as stormdrive and stormconsole do.
