# Plugin UI

A supervised process that serves its own web UI can appear as a tab in
stormd's web console, and can put its own numbers on its dashboard card,
without any change to stormd. Written from `api.rs` (`proxy_plugin`),
`components.rs` and `main.rs` at v0.8.0 (refreshed 2026-10-09).

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
- Methods: any — the request method is passed through unchanged.
- Request: every header the browser sent, minus hop-by-hop ones
  (`Connection` and what it lists, `Keep-Alive`, `Transfer-Encoding`,
  `Upgrade`, `TE`, `Trailer`, `Proxy-*`) and `Host`/`Content-Length`; the
  body as bytes, so binary uploads survive — up to 2 MB (axum's default
  request-body limit; a larger body is a 413 from stormd). A plugin's own
  `Authorization: Bearer …` (stormstorage's `api_token`, say) reaches the
  plugin. stormd's own credentials stop at the proxy: an `Authorization`
  carrying stormd's bearer token (`auth_token` or `token_file`), and the
  `stormd_session` cookie.
- Response: the status and every upstream header minus hop-by-hop ones —
  `Set-Cookie`, `Location`, caching and encoding headers included — except a
  `Set-Cookie` for `stormd_session`, which is dropped. Redirects are not
  followed; the browser gets the `Location`. An absolute `Location` (`/login`)
  is not rewritten to the `/ui/proxy/{name}/` prefix, so use relative ones.
- No WebSocket upgrade (#53, with the `Location` and body-limit gaps above).
- A plugin that cannot be reached answers 500 with `{"error": "proxy: ..."}`.
- With stormd auth on, `/ui/proxy/*` requires a session (the browser's
  session cookie rides along with the iframe's requests) or stormd's bearer.
  A request carrying a plugin's bearer is let in by the session cookie: a
  bearer that is not stormd's is not a failure, it is just not stormd's.

So a plugin UI should use relative URLs (it is mounted under a prefix), name
its cookies anything but `stormd_session`, and use plain `fetch` for anything
live.

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
