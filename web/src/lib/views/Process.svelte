<script>
  // Per-process detail: its summary card plus a live console, one page.
  import { route } from '../router.svelte.js'
  import { feed } from '../stores.svelte.js'
  import ComponentCard from 'stormview/components/ComponentCard.svelte'
  import { post } from '../api.js'
  import Terminal from './Terminal.svelte'

  let name = $derived(route.current.params.name)
  let component = $derived(feed.components.find((c) => c.id === 'process:' + name))
</script>

<div class="content head">
  {#if component}
    <div class="card-col"><ComponentCard {component} resolve={(id) => feed.components.find((c) => c.id === id)} invoke={(a) => post(a.path)} /></div>
  {:else}
    <div class="missing">Process “{name}” not found.</div>
  {/if}
</div>

{#key name}
  <Terminal preselect={name} />
{/key}

<style>
  .head { padding-bottom: 0; }
  .card-col { max-width: 480px; }
  .missing { color: var(--text-faint); }
</style>
