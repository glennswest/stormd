<script>
  import { nav, login } from '../stores.svelte.js'

  let password = $state('')
  let error = $state('')
  let busy = $state(false)

  async function submit(e) {
    e.preventDefault()
    if (!password || busy) return
    busy = true
    error = ''
    try {
      await login(password)
    } catch (err) {
      error = err.message || 'login failed'
      password = ''
    } finally {
      busy = false
    }
  }
</script>

<div class="wrap">
  <form class="card" onsubmit={submit}>
    <div class="brand">{nav.container}</div>
    <div class="sub">stormd</div>
    <!-- svelte-ignore a11y_autofocus -->
    <input
      type="password"
      placeholder="Password"
      bind:value={password}
      autofocus
      autocomplete="current-password"
    />
    {#if error}<div class="error">{error}</div>{/if}
    <button type="submit" disabled={busy || !password}>Sign in</button>
  </form>
</div>

<style>
  .wrap {
    min-height: 100vh;
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--bg);
  }
  .card {
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 32px;
    width: min(320px, 90vw);
    display: flex;
    flex-direction: column;
    gap: 12px;
    text-align: center;
  }
  .brand {
    font-size: 24px;
    font-weight: 700;
    color: var(--brand);
    letter-spacing: -0.5px;
  }
  .sub {
    font-size: 12px;
    color: var(--text-faint);
    text-transform: uppercase;
    letter-spacing: 1px;
    margin-bottom: 8px;
  }
  input { text-align: center; }
  .error { color: var(--error); font-size: 12px; }
</style>
