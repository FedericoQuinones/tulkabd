<script lang="ts">
  interface Props {
    text: string;
    error?: boolean;
  }
  let { text, error = false }: Props = $props();
</script>

<div class="toast" class:error role="status">{text}</div>

<style>
  .toast {
    position: absolute;
    right: 14px;
    top: 8px;
    z-index: 4;
    max-width: 360px;
    padding: 10px 14px;
    background: var(--raised);
    border: 1px solid var(--control);
    border-radius: var(--radius-card);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
    font-weight: 600;
  }
  .toast.error {
    color: var(--negative);
    font-weight: 400;
  }
</style>
