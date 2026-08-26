<script>
  // The feed rendered as a relational grid: rows are components, has_one /
  // has_many edges expand into nested grids, belongs_to stays a chip, and a
  // multi-selection gets bulk actions. This is DataGrid + RelationPicker
  // wired to the stormview contract — the shape stormdrive and stormconsole
  // will reuse.
  import DataGrid from './DataGrid.svelte'
  import RelationPicker from './RelationPicker.svelte'
  import { navigate } from '../router.svelte.js'
  import { post } from '../api.js'

  let { components = [] } = $props()
  let selected = $state([])
  let busy = $state(false)

  const byId = $derived(new Map(components.map((c) => [c.id, c])))
  const resolve = (id) => byId.get(id)

  // Top level: components that belong to nothing present in the feed —
  // everything else is reachable by expanding relations.
  const roots = $derived(
    components.filter(
      (c) =>
        !(c.relations || []).some(
          (r) => r.kind === 'belongs_to' && r.targets.some((t) => byId.has(t))
        )
    )
  )

  const columns = [
    { key: 'health', label: '', render: 'health', width: '90px' },
    { key: 'label', label: 'Component' },
    { key: 'kind', label: 'Kind', width: '90px' },
    { key: 'detail', label: 'Detail' },
    { key: 'metrics', label: 'Metrics', render: 'metrics', sortable: false },
    { key: 'actions', label: 'Actions', render: 'actions', sortable: false },
  ]

  // A component's downward edges (has_one / has_many) become nested grids,
  // ancestors excluded so cycles terminate.
  function childSections(row, ancestors) {
    return (row.relations || [])
      .filter((r) => r.kind !== 'belongs_to')
      .map((r) => ({
        title: r.name,
        rows: r.targets
          .filter((t) => !ancestors.has(t))
          .map(resolve)
          .filter(Boolean),
        getChildren: (child) => childSections(child, new Set([...ancestors, row.id])),
      }))
      .filter((s) => s.rows.length)
  }

  function openRow(row) {
    if (row.link) navigate(row.link)
  }

  async function invoke(row, action) {
    if (action.danger && !confirm(`${action.label} ${row.label}?`)) return
    try {
      await post(action.path)
    } catch (e) {
      console.error(e)
    }
  }

  // Bulk actions: whichever action ids every selected component offers
  // enabled right now.
  const bulkActions = $derived.by(() => {
    const rows = selected.map(resolve).filter(Boolean)
    if (rows.length < 2) return []
    const ids = ['start', 'stop', 'restart']
    return ids
      .map((id) => {
        const acts = rows
          .map((r) => (r.actions || []).find((a) => a.id === id && a.enabled))
          .filter(Boolean)
        return acts.length === rows.length ? { id, label: acts[0].label, acts } : null
      })
      .filter(Boolean)
  })

  async function runBulk(bulk) {
    if (!confirm(`${bulk.label} ${bulk.acts.length} components?`)) return
    busy = true
    for (const a of bulk.acts) {
      try {
        await post(a.path)
      } catch (e) {
        console.error(e)
      }
    }
    busy = false
    selected = []
  }
</script>

{#if bulkActions.length}
  <div class="bulk-bar">
    <span>{selected.length} selected</span>
    {#each bulkActions as b}
      <button disabled={busy} onclick={() => runBulk(b)}>{b.label} all</button>
    {/each}
    <button class="clear" onclick={() => (selected = [])}>Clear</button>
  </div>
{/if}

<DataGrid
  {columns}
  rows={roots}
  getChildren={(row) => childSections(row, new Set([row.id]))}
  selectable="multi"
  bind:selected
  onaction={invoke}
  onrowclick={openRow}
/>

<div class="pickers">
  {#each roots as root}
    {#each (root.relations || []).filter((r) => r.kind === 'has_many') as rel}
      <RelationPicker relation={rel} {resolve} />
    {/each}
  {/each}
</div>

<style>
  .bulk-bar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 12px;
    margin-bottom: 10px;
    background: var(--accent-bg);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    font-size: 13px;
  }
  .bulk-bar .clear { margin-left: auto; }
  .pickers {
    display: flex;
    gap: 8px;
    margin-top: 10px;
    align-items: center;
  }
</style>
