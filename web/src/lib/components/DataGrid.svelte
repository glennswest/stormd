<script>
  // The reusable grid. Deliberately knows nothing about stormd — columns,
  // rows, nesting, and selection are all injected — so stormdrive and
  // stormconsole can lift it as-is.
  //
  //   columns:     [{ key, label, width?, render?, sortable? }]
  //                render: 'text' (default) | 'mono' | 'health' | 'metrics'
  //                        | 'actions' | (row) => string
  //   rows:        objects with a stable `id`; cells read row[key]
  //   getChildren: (row) => [{ title, rows, columns?, getChildren? }]
  //                non-empty ⇒ the row gets an expander and nested grids
  //   selectable:  null | 'single' | 'multi'  (bind:selected — array of ids)
  //   onaction:    (row, action) — invoked from 'actions' cells
  //   onrowclick:  (row)
  import HealthDot from './HealthDot.svelte'
  import DataGrid from './DataGrid.svelte'

  let {
    columns = [],
    rows = [],
    getChildren = null,
    selectable = null,
    selected = $bindable([]),
    onaction = null,
    onrowclick = null,
    level = 0,
  } = $props()

  let sortKey = $state(null)
  let sortDir = $state(1)
  let expanded = $state({})

  let sorted = $derived.by(() => {
    if (!sortKey) return rows
    const key = sortKey
    const dir = sortDir
    return [...rows].sort((a, b) => {
      const x = a[key]
      const y = b[key]
      if (x == null) return 1
      if (y == null) return -1
      if (typeof x === 'number' && typeof y === 'number') return (x - y) * dir
      return String(x).localeCompare(String(y)) * dir
    })
  })

  function sortBy(col) {
    if (col.sortable === false) return
    if (sortKey === col.key) sortDir = -sortDir
    else {
      sortKey = col.key
      sortDir = 1
    }
  }

  function toggleExpand(id) {
    expanded[id] = !expanded[id]
  }

  function toggleSelect(id) {
    if (selectable === 'single') {
      selected = selected.includes(id) ? [] : [id]
    } else if (selectable === 'multi') {
      selected = selected.includes(id)
        ? selected.filter((s) => s !== id)
        : [...selected, id]
    }
  }

  function toggleSelectAll() {
    selected = selected.length === rows.length ? [] : rows.map((r) => r.id)
  }

  function childrenOf(row) {
    if (!getChildren) return []
    return (getChildren(row) || []).filter((s) => s.rows?.length)
  }

  const extraCols = $derived((getChildren ? 1 : 0) + (selectable ? 1 : 0))
</script>

<div class="grid-wrap" class:nested={level > 0}>
  <table>
    <thead>
      <tr>
        {#if getChildren}<th class="ctl"></th>{/if}
        {#if selectable}
          <th class="ctl">
            {#if selectable === 'multi'}
              <input
                type="checkbox"
                checked={rows.length > 0 && selected.length === rows.length}
                onchange={toggleSelectAll}
              />
            {/if}
          </th>
        {/if}
        {#each columns as col}
          <th
            style={col.width ? `width:${col.width}` : ''}
            class:sortable={col.sortable !== false}
            onclick={() => sortBy(col)}
          >
            {col.label}
            {#if sortKey === col.key}<span class="dir">{sortDir > 0 ? '▼' : '▲'}</span>{/if}
          </th>
        {/each}
      </tr>
    </thead>
    <tbody>
      {#each sorted as row (row.id)}
        {@const kids = childrenOf(row)}
        <tr
          class:selected={selected.includes(row.id)}
          class:clickable={!!onrowclick}
          onclick={() => onrowclick?.(row)}
        >
          {#if getChildren}
            <td class="ctl">
              {#if kids.length}
                <button
                  class="expander"
                  onclick={(e) => {
                    e.stopPropagation()
                    toggleExpand(row.id)
                  }}>{expanded[row.id] ? '▾' : '▸'}</button
                >
              {/if}
            </td>
          {/if}
          {#if selectable}
            <td class="ctl" onclick={(e) => e.stopPropagation()}>
              <input
                type="checkbox"
                checked={selected.includes(row.id)}
                onchange={() => toggleSelect(row.id)}
              />
            </td>
          {/if}
          {#each columns as col}
            {@const v = row[col.key]}
            <td>
              {#if typeof col.render === 'function'}
                {col.render(row)}
              {:else if col.render === 'health'}
                <span class="health-cell"><HealthDot health={v} size={9} /> {v}</span>
              {:else if col.render === 'metrics'}
                <span class="metrics-cell">
                  {#each v || [] as m}
                    <span class="m">
                      <span class="ml">{m.label}</span>
                      <span class="mv {m.tone || ''}">{m.value}{m.unit || ''}</span>
                    </span>
                  {/each}
                </span>
              {:else if col.render === 'actions'}
                <span class="actions-cell" onclick={(e) => e.stopPropagation()}>
                  {#each v || [] as a}
                    <button
                      class:danger={a.danger}
                      disabled={!a.enabled}
                      onclick={() => onaction?.(row, a)}>{a.label}</button
                    >
                  {/each}
                </span>
              {:else if col.render === 'mono'}
                <span class="mono">{v ?? ''}</span>
              {:else}
                {v ?? ''}
              {/if}
            </td>
          {/each}
        </tr>
        {#if expanded[row.id] && kids.length}
          <tr class="child-row">
            <td colspan={columns.length + extraCols}>
              {#each kids as section}
                <div class="child-section">
                  <div class="child-title">{section.title}</div>
                  <DataGrid
                    columns={section.columns || columns}
                    rows={section.rows}
                    getChildren={section.getChildren ?? getChildren}
                    {selectable}
                    bind:selected
                    {onaction}
                    {onrowclick}
                    level={level + 1}
                  />
                </div>
              {/each}
            </td>
          </tr>
        {/if}
      {/each}
    </tbody>
  </table>
</div>

<style>
  .grid-wrap {
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    box-shadow: var(--shadow);
    overflow-x: auto;
  }
  .grid-wrap.nested {
    background: var(--panel-raised);
    border-color: var(--border);
  }
  table { width: 100%; border-collapse: collapse; }
  th {
    text-align: left;
    padding: 9px 12px;
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-faint);
    border-bottom: 1px solid var(--border);
    white-space: nowrap;
    user-select: none;
  }
  th.sortable { cursor: pointer; }
  th.sortable:hover { color: var(--text-dim); }
  .dir { color: var(--accent); }
  td {
    padding: 8px 12px;
    font-size: 13px;
    border-bottom: 1px solid var(--panel-raised);
    vertical-align: middle;
  }
  .nested td { border-bottom-color: var(--border); }
  tbody tr:hover:not(.child-row) { background: var(--panel-raised); }
  .nested tbody tr:hover:not(.child-row) { background: var(--nav-hover); }
  tr.selected { background: var(--accent-bg); }
  tr.clickable { cursor: pointer; }
  th.ctl, td.ctl { width: 28px; padding: 8px 4px 8px 10px; }
  .expander {
    background: none;
    border: none;
    color: var(--text-dim);
    padding: 0 4px;
    font-size: 12px;
    cursor: pointer;
  }
  .child-row > td {
    padding: 4px 12px 12px 34px;
    background: color-mix(in srgb, var(--panel-raised) 40%, transparent);
  }
  .child-section { margin-top: 8px; }
  .child-title {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.6px;
    color: var(--text-faint);
    margin-bottom: 4px;
  }
  .health-cell { display: inline-flex; align-items: center; gap: 6px; font-size: 12px; }
  .metrics-cell { display: inline-flex; gap: 14px; flex-wrap: wrap; }
  .m { display: inline-flex; gap: 4px; align-items: baseline; }
  .ml { font-size: 10px; text-transform: uppercase; color: var(--text-faint); }
  .mv { font-family: var(--mono); font-size: 12px; font-weight: 600; }
  .mv.ok { color: var(--ok); }
  .mv.warn { color: var(--warn-strong); }
  .mv.error { color: var(--error); }
  .mv.muted { color: var(--text-dim); font-weight: 400; }
  .mv.accent { color: var(--accent); }
  .actions-cell { display: inline-flex; gap: 4px; }
  .actions-cell button { padding: 2px 10px; font-size: 11px; }
  .actions-cell button.danger {
    background: var(--error-bg);
    border-color: var(--error-border);
    color: var(--error);
  }
  input[type='checkbox'] { accent-color: var(--accent); }
</style>
