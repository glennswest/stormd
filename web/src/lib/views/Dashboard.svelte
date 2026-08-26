<script>
  // The dashboard is a grouped rendering of the component-summary feed —
  // it holds no model of its own. New kinds land in "everything else"
  // until they earn a section of their own.
  import { feed } from '../stores.svelte.js'
  import ComponentCard from '../components/ComponentCard.svelte'
  import ComponentGrid from '../components/ComponentGrid.svelte'
  import MemChart from '../components/MemChart.svelte'

  let mode = $state(
    (() => {
      try {
        return localStorage.getItem('stormd-dash-mode') || 'cards'
      } catch {
        return 'cards'
      }
    })()
  )

  function setMode(m) {
    mode = m
    try {
      localStorage.setItem('stormd-dash-mode', m)
    } catch {}
  }

  const sections = [
    { title: 'System', kinds: ['system', 'storage', 'logs'] },
    { title: 'Processes', kinds: ['process', 'plugin'] },
    { title: 'Cron', kinds: ['cron'] },
    { title: 'Updates', kinds: ['updater'] },
  ]

  let grouped = $derived(
    sections
      .map((s) => ({
        ...s,
        items: feed.components.filter((c) => s.kinds.includes(c.kind)),
      }))
      .concat([
        {
          title: 'Other',
          kinds: [],
          items: feed.components.filter(
            (c) => !sections.some((s) => s.kinds.includes(c.kind))
          ),
        },
      ])
      .filter((s) => s.items.length > 0)
  )
</script>

<div class="content">
  {#if !feed.loaded}
    <div class="empty">Connecting…</div>
  {:else}
    <div class="mode-bar">
      <button class:active={mode === 'cards'} onclick={() => setMode('cards')}>Cards</button>
      <button class:active={mode === 'grid'} onclick={() => setMode('grid')}>Grid</button>
    </div>
    {#if mode === 'grid'}
      <ComponentGrid components={feed.components} />
    {:else}
      {#each grouped as section}
        <h2>{section.title}</h2>
        <div class="grid">
          {#each section.items as c (c.id)}
            <ComponentCard component={c} />
          {/each}
          {#if section.title === 'System'}
            <MemChart />
          {/if}
        </div>
      {/each}
    {/if}
  {/if}
</div>

<style>
  h2 {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-dim);
    text-transform: uppercase;
    letter-spacing: 0.8px;
    margin: 24px 0 10px;
  }
  h2::after {
    content: '';
    flex: 1;
    height: 1px;
    background: var(--border);
  }
  h2:first-of-type { margin-top: 0; }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
    gap: 16px;
  }
  .empty { color: var(--text-faint); padding: 40px; text-align: center; }
  .mode-bar { display: flex; gap: 4px; margin-bottom: 12px; }
  .mode-bar button { padding: 3px 12px; font-size: 12px; color: var(--text-dim); }
  .mode-bar button.active { color: var(--text); background: var(--nav-active); }
</style>
