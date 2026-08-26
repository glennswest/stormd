<script>
  import { route } from './lib/router.svelte.js'
  import { auth, checkAuth, startFeed } from './lib/stores.svelte.js'
  import Nav from './lib/components/Nav.svelte'
  import Dashboard from './lib/views/Dashboard.svelte'
  import Logs from './lib/views/Logs.svelte'
  import Terminal from './lib/views/Terminal.svelte'
  import Process from './lib/views/Process.svelte'
  import Ext from './lib/views/Ext.svelte'
  import GridView from './lib/views/GridView.svelte'
  import Login from './lib/views/Login.svelte'

  checkAuth().then(() => {
    if (!auth.required || auth.authenticated) startFeed()
  })

  const views = {
    dashboard: Dashboard,
    logs: Logs,
    terminal: Terminal,
    process: Process,
    ext: Ext,
    grid: GridView,
  }

  let View = $derived(views[route.current.name] || Dashboard)
</script>

{#if !auth.checked}
  <!-- one tick while the session check runs -->
{:else if auth.required && !auth.authenticated}
  <Login />
{:else}
  <Nav />
  {#key route.current.name + (route.current.params.name || '') + route.current.query.toString()}
    <View />
  {/key}
{/if}
