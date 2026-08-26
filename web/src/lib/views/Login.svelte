<script>
  import LoginPanel from 'stormview/components/LoginPanel.svelte'
  import { nav, login } from '../stores.svelte.js'
</script>

<LoginPanel
  title={nav.container}
  subtitle="stormd · sign in"
  onsubmit={(username, password) => login(username, password)}
/>
