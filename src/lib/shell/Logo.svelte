<script lang="ts">
  let { size = 28 }: { size?: number } = $props();
</script>

<svg width={size} height={size} viewBox="0 0 64 64" role="img" aria-label="tulkabd">
  <path
    d="M8 33c0-10 8.6-17 19.5-17 7.6 0 13.3 3.4 16.6 8.6L55 17.5c1.4-.9 3.2.4 2.8 2-1.3 4.6-1.3 9.4 0 14 .4 1.6-1.4 2.9-2.8 2L44 28.5C41 39.6 35 47 25.5 47 15.5 47 8 41.5 8 33z"
    fill="var(--accent)"
    transform="rotate(-8 32 32)"
  />
  <circle cx="19" cy="29" r="5.2" fill="#fff" />
  <circle cx="18" cy="29.6" r="2.6" fill="#131217" />
  <circle cx="16.9" cy="28.4" r=".9" fill="#fff" />
  <ellipse cx="22.5" cy="38" rx="3.2" ry="2" fill="var(--blush)" opacity=".9" />
  <path d="M14 37.5c1.4 1.3 3 1.3 4.4 0" fill="none" stroke="#131217" stroke-width="2" stroke-linecap="round" />
</svg>
