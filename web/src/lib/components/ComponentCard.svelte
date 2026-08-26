<script>
  // Renders any ComponentSummary from /api/v1/components — the card knows
  // nothing about kinds beyond an icon; a new subsystem needs no UI work.
  import HealthDot from './HealthDot.svelte'
  import { post } from '../api.js'

  let { component } = $props()
  let busy = $state(false)

  const icons = {
    system: '⛈',
    process: '▸',
    plugin: '⚙',
    cron: '↻',
    storage: '◫',
    logs: '≡',
    updater: '⇪',
  }

  async function invoke(action) {
    if (action.danger && !confirm(`${action.label} ${component.label}?`)) return
    busy = true
    try {
      await post(action.path)
    } catch (e) {
      console.error(e)
    } finally {
      busy = false
    }
  }

  function toneClass(tone) {
    return tone || 'plain'
  }
</script>

<div class="card" class:error={component.health === 'error'} class:warn={component.health === 'warn'}>
  <div class="head">
    <HealthDot health={component.health} />
    <span class="icon">{icons[component.kind] || '•'}</span>
    {#if component.link}
      <a class="label" href={component.link}>{component.label}</a>
    {:else}
      <span class="label">{component.label}</span>
    {/if}
    <span class="kind">{component.kind}</span>
  </div>

  <div class="detail">{component.detail}</div>

  {#if component.metrics?.length}
    <div class="metrics">
      {#each component.metrics as m}
        <div class="metric">
          <span class="mlabel">{m.label}</span>
          <span class="mvalue {toneClass(m.tone)}">{m.value}{m.unit || ''}</span>
        </div>
      {/each}
    </div>
  {/if}

  {#if component.actions?.length}
    <div class="actions">
      {#each component.actions as a}
        <button
          class:ok={a.id === 'start'}
          class:danger={a.danger}
          class:warn={a.id === 'restart'}
          disabled={!a.enabled || busy}
          onclick={() => invoke(a)}>{a.label}</button
        >
      {/each}
    </div>
  {/if}
</div>

<style>
  .card {
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 14px 16px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    transition: border-color 0.2s;
  }
  .card:hover { border-color: var(--border-strong); }
  .card.error { border-color: var(--error-border); }
  .card.warn { border-color: var(--warn-border); }

  .head { display: flex; align-items: center; gap: 8px; min-width: 0; }
  .icon { color: var(--text-faint); font-size: 13px; }
  .label {
    font-weight: 600;
    font-size: 14px;
    color: var(--text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  a.label:hover { color: var(--accent); text-decoration: none; }
  .kind {
    margin-left: auto;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-ghost);
    background: var(--panel-raised);
    padding: 2px 7px;
    border-radius: 9px;
  }

  .detail { font-size: 12px; color: var(--text-dim); }

  .metrics {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 18px;
    padding-top: 2px;
  }
  .metric { display: flex; flex-direction: column; gap: 1px; }
  .mlabel {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-faint);
  }
  .mvalue { font-size: 15px; font-weight: 600; font-family: var(--mono); }
  .mvalue.ok { color: var(--ok); }
  .mvalue.warn { color: var(--warn-strong); }
  .mvalue.error { color: var(--error); }
  .mvalue.muted { color: var(--text-dim); font-weight: 400; }
  .mvalue.accent { color: var(--accent); }

  .actions { display: flex; gap: 6px; padding-top: 4px; }
  .actions button { padding: 4px 12px; font-size: 12px; }
</style>
