<script>
  let { health = 'unknown', size = 10 } = $props()
</script>

<span
  class="dot {health}"
  style="width:{size}px;height:{size}px"
  title={health}
></span>

<style>
  .dot {
    display: inline-block;
    border-radius: 50%;
    flex-shrink: 0;
  }
  .dot.ok { background: var(--ok); box-shadow: 0 0 6px rgba(80, 250, 123, 0.5); }
  .dot.warn { background: var(--warn-strong); box-shadow: 0 0 6px rgba(240, 160, 48, 0.5); }
  .dot.error { background: var(--error); box-shadow: 0 0 6px rgba(233, 69, 96, 0.6); animation: pulse 1.2s infinite; }
  .dot.idle { background: var(--text-ghost); }
  .dot.unknown { background: var(--text-ghost); opacity: 0.5; }

  @keyframes pulse {
    50% { opacity: 0.4; }
  }
</style>
