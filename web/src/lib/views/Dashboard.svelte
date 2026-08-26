<script>
  // The dashboard is a grouped rendering of the component-summary feed —
  // it holds no model of its own. New kinds land in "everything else"
  // until they earn a section of their own.
  import { feed } from '../stores.svelte.js'
  import ComponentCard from '../components/ComponentCard.svelte'
  import MemChart from '../components/MemChart.svelte'

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
</div>

<style>
  h2 {
    font-size: 12px;
    font-weight: 600;
    color: var(--text-faint);
    text-transform: uppercase;
    letter-spacing: 0.8px;
    margin: 20px 0 10px;
  }
  h2:first-of-type { margin-top: 0; }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(290px, 1fr));
    gap: 12px;
  }
  .empty { color: var(--text-faint); padding: 40px; text-align: center; }
</style>
