<script>
  // Plugin page: nav chrome around the plugin's own UI, reverse-proxied by
  // stormd at /ui/proxy/{name}/.
  import { route } from '../router.svelte.js'

  let name = $derived(route.current.params.name)
</script>

{#key name}
  <iframe class="plugin-frame" title={name} src={'/ui/proxy/' + encodeURIComponent(name) + '/'}
  ></iframe>
{/key}

<style>
  .plugin-frame {
    width: 100%;
    height: calc(100vh - var(--nav-h));
    border: none;
    display: block;
  }
</style>
