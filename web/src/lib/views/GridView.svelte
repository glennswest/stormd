<script>
  // A grid rooted at one component — where a card's ⊞ lands. With ?rel= the
  // top rows are that relationship's targets; without it, the component
  // itself is the single expandable root.
  import { route } from '../router.svelte.js'
  import { feed } from '../stores.svelte.js'
  import ComponentGrid from '../components/ComponentGrid.svelte'
  import HealthDot from '../components/HealthDot.svelte'

  const id = $derived(route.current.query.get('id'))
  const rel = $derived(route.current.query.get('rel'))
  const root = $derived(feed.components.find((c) => c.id === id))

  const rootIds = $derived.by(() => {
    if (!root) return []
    if (rel) {
      const r = (root.relations || []).find((x) => x.name === rel)
      if (r) return r.targets
    }
    return [root.id]
  })
</script>

<div class="content">
  {#if !feed.loaded}
    <div class="empty">Connecting…</div>
  {:else if !root}
    <div class="empty">Component “{id}” not found. <a href="#/">Back to dashboard</a></div>
  {:else}
    <div class="head">
      <a href="#/" class="back">← Dashboard</a>
      <h1>
        <HealthDot health={root.health} />
        {root.label}
        {#if rel}<span class="rel">· {rel}</span>{/if}
      </h1>
    </div>
    <ComponentGrid components={feed.components} {rootIds} />
  {/if}
</div>

<style>
  .head { display: flex; align-items: baseline; gap: 16px; margin-bottom: 14px; }
  .back { font-size: 13px; color: var(--text-dim); }
  .back:hover { color: var(--accent); text-decoration: none; }
  h1 {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 16px;
    font-weight: 600;
  }
  .rel { color: var(--text-dim); font-weight: 400; }
  .empty { color: var(--text-faint); padding: 40px; text-align: center; }
</style>
