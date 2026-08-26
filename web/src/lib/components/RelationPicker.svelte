<script>
  // "Select from a relationship": a has_many edge rendered as a picker.
  // Resolve maps target ids to components from the feed; picking one calls
  // onpick(component) — the default navigates to its page.
  import { navigate } from '../router.svelte.js'

  let { relation, resolve, onpick = null } = $props()

  let targets = $derived(
    relation.targets.map((id) => resolve(id)).filter(Boolean)
  )

  function pick(e) {
    const id = e.target.value
    if (!id) return
    const c = resolve(id)
    e.target.value = ''
    if (!c) return
    if (onpick) onpick(c)
    else if (c.link) navigate(c.link)
  }
</script>

<select class="rel-pick" onchange={pick} onclick={(e) => e.stopPropagation()}>
  <option value="">{relation.name} ({targets.length})</option>
  {#each targets as t}
    <option value={t.id}>{t.label}</option>
  {/each}
</select>

<style>
  .rel-pick {
    padding: 2px 6px;
    font-size: 11px;
    color: var(--text-dim);
    max-width: 140px;
  }
</style>
