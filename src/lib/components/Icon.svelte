<script lang="ts" module>
  export type IconName =
    | "search"
    | "plus"
    | "close"
    | "chevron-down"
    | "chevron-right"
    | "pin"
    | "folder"
    | "folder-open"
    | "settings"
    | "keyboard"
    | "sidebar"
    | "arrow-right"
    | "resume"
    | "error"
    | "alert"
    | "warning"
    | "panel";
</script>

<script lang="ts">
  // Inline stroke icons from the design boards (16×16 grid, currentColor).
  let { name, small = false, color }: { name: IconName; small?: boolean; color?: string } = $props();
</script>

<svg
  class="i"
  class:s={small}
  viewBox="0 0 16 16"
  fill="none"
  stroke={color ?? "currentColor"}
  stroke-width="1.6"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  {#if name === "search"}
    <circle cx="7" cy="7" r="4.5" /><path d="M10.5 10.5 14 14" />
  {:else if name === "plus"}
    <path d="M8 3v10M3 8h10" />
  {:else if name === "close"}
    <path d="M4 4l8 8M12 4l-8 8" />
  {:else if name === "chevron-down"}
    <path d="M4 6l4 4 4-4" />
  {:else if name === "chevron-right"}
    <path d="M6 4l4 4-4 4" />
  {:else if name === "pin"}
    <path d="M6 2h4l-.5 4 2.5 2.5H4L6.5 6zM8 8.5V14" />
  {:else if name === "folder"}
    <path
      d="M2 4.5A1.5 1.5 0 0 1 3.5 3h3l1.5 1.5h4.5A1.5 1.5 0 0 1 14 6v5.5a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 2 11.5z"
    />
  {:else if name === "folder-open"}
    <path d="M2 11.5V4.5A1.5 1.5 0 0 1 3.5 3h3l1.5 1.5h4A1.5 1.5 0 0 1 13.5 6v1.2" />
    <path d="M2.2 12.6 4 8a1.5 1.5 0 0 1 1.4-1H14l-1.9 5a1.5 1.5 0 0 1-1.4 1H3.1a1 1 0 0 1-.9-1.4z" />
  {:else if name === "settings"}
    <path d="M2 4h7M12 4h2M2 12h2M7 12h7" /><circle cx="10.5" cy="4" r="1.5" /><circle cx="5.5" cy="12" r="1.5" />
  {:else if name === "keyboard"}
    <rect x="1.5" y="4" width="13" height="8" rx="1.5" /><path d="M4 7h.01M6.5 7h.01M9 7h.01M11.5 7h.01M5 9.5h6" />
  {:else if name === "sidebar"}
    <rect x="2" y="3" width="12" height="10" rx="2" /><path d="M6 3v10" />
  {:else if name === "arrow-right"}
    <path d="M3 8h10M9 4l4 4-4 4" />
  {:else if name === "resume"}
    <path d="M13 8a5 5 0 1 1-1.5-3.6M13 2.5V5h-2.5" />
  {:else if name === "error"}
    <circle cx="8" cy="8" r="6" /><path d="M6 6l4 4M10 6l-4 4" />
  {:else if name === "alert"}
    <circle cx="8" cy="8" r="6" /><path d="M8 5v3.5M8 11h.01" />
  {:else if name === "warning"}
    <path d="M8 2.5 14 13H2z" /><path d="M8 6.5v3M8 11.2v.01" />
  {:else if name === "panel"}
    <rect x="2" y="3" width="12" height="10" rx="2" />
  {/if}
</svg>
