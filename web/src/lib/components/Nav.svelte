<script>
  import { route } from '../router.svelte.js'
  import { auth, feed, nav, logout } from '../stores.svelte.js'
  import { THEMES, theme, applyTheme } from '../theme.svelte.js'

  const links = [
    { href: '#/', label: 'Dashboard', name: 'dashboard' },
    { href: '#/terminal', label: 'Terminal', name: 'terminal' },
    { href: '#/logs', label: 'Logs', name: 'logs' },
  ]
</script>

<nav>
  <a class="brand" href="#/">{nav.container}</a>
  <div class="links">
    {#each links as l}
      <a href={l.href} class:active={route.current.name === l.name}>{l.label}</a>
    {/each}
    {#each nav.plugins as p}
      <a
        href={'#/ext/' + encodeURIComponent(p.name)}
        class:active={route.current.name === 'ext' && route.current.params.name === p.name}
        >{p.label}</a
      >
    {/each}
  </div>
  <span class="right">
    <select
      class="theme-pick"
      title="Theme"
      value={theme.current}
      onchange={(e) => applyTheme(e.target.value)}
    >
      {#each THEMES as t}
        <option value={t.id}>{t.label}</option>
      {/each}
    </select>
    <span class="live" class:on={feed.connected} title={feed.connected ? 'live' : 'reconnecting'}></span>
    stormd
    {#if auth.required}
      <button class="signout" title="Sign out" onclick={logout}>⏻</button>
    {/if}
  </span>
</nav>

<style>
  nav {
    background: var(--panel);
    border-bottom: 1px solid var(--border);
    padding: 0 20px;
    display: flex;
    align-items: center;
    height: var(--nav-h);
    gap: 8px;
    overflow-x: auto;
  }
  .brand {
    font-size: 18px;
    font-weight: 700;
    color: var(--brand);
    margin-right: 24px;
    letter-spacing: -0.5px;
    white-space: nowrap;
  }
  .brand:hover { text-decoration: none; filter: brightness(1.15); }
  .links { display: flex; gap: 4px; }
  .links a {
    padding: 8px 16px;
    border-radius: var(--radius-sm);
    font-size: 13px;
    font-weight: 500;
    color: var(--text-dim);
    transition: all 0.15s;
    white-space: nowrap;
  }
  .links a:hover { color: var(--text); background: var(--nav-hover); text-decoration: none; }
  .links a.active { color: var(--text); background: var(--nav-active); }
  .right {
    margin-left: auto;
    font-size: 12px;
    color: var(--text-dim);
    font-weight: 500;
    display: flex;
    align-items: center;
    gap: 6px;
    white-space: nowrap;
  }
  .live {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--text-ghost);
    transition: background 0.3s;
  }
  .live.on { background: var(--ok); box-shadow: 0 0 6px var(--ok); }
  .theme-pick {
    padding: 3px 6px;
    font-size: 12px;
    background: var(--panel-raised);
    color: var(--text-dim);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
  .signout {
    padding: 2px 8px;
    font-size: 13px;
    background: none;
    border: 1px solid var(--border);
    color: var(--text-dim);
  }
  .signout:hover { color: var(--error); border-color: var(--error-border); }
</style>
