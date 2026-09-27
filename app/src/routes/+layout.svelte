<script lang="ts">
  import "$lib/tokens.css";
  // After the tokens, because every shape in it is built from them.
  import "$lib/components.css";
  let { children } = $props();
</script>

<svelte:head>
  <title>Anomaly</title>
</svelte:head>

{@render children()}
