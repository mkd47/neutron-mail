<script lang="ts">
  // Small inline SVG icon set (no external icon fonts or network loads).
  let { name, size = 18 }: { name: string; size?: number } = $props();
  const paths: Record<string, string> = {
    inbox: "M4 4h16v10h-5l-1.5 2h-3L9 14H4z M4 14v6h16v-6",
    sent: "M3 11l18-8-8 18-2-8z",
    drafts: "M4 20h4L19 9l-4-4L4 16z",
    trash: "M5 7h14 M9 7V4h6v3 M7 7l1 13h8l1-13",
    archive: "M3 4h18v4H3z M5 8v12h14V8 M10 12h4",
    junk: "M12 3l9 16H3z M12 10v4 M12 17v.5",
    folder: "M3 6h7l2 2h9v11H3z",
    star: "M12 3l2.7 5.6 6.1.9-4.4 4.3 1 6.1L12 17l-5.4 2.9 1-6.1-4.4-4.3 6.1-.9z",
    compose: "M4 20h4L19 9l-4-4L4 16z M14 6l4 4",
    search: "M10.5 4a6.5 6.5 0 1 0 0 13 6.5 6.5 0 0 0 0-13z M15.5 15.5L21 21",
    sync: "M20 11a8 8 0 0 0-14.5-4.5L4 8 M4 4v4h4 M4 13a8 8 0 0 0 14.5 4.5L20 16 M20 20v-4h-4",
    sun: "M12 7a5 5 0 1 0 0 10 5 5 0 0 0 0-10z M12 1v3 M12 20v3 M1 12h3 M20 12h3 M4.2 4.2l2 2 M17.8 17.8l2 2 M4.2 19.8l2-2 M17.8 6.2l2-2",
    moon: "M21 13A9 9 0 1 1 11 3a7 7 0 0 0 10 10z",
    clip: "M21 11l-8.5 8.5a5 5 0 0 1-7-7L14 4a3.5 3.5 0 0 1 5 5l-8.5 8.5a2 2 0 0 1-3-3L15 7",
    close: "M6 6l12 12 M18 6L6 18",
    shield: "M12 3l8 3v6c0 5-3.5 8-8 9-4.5-1-8-4-8-9V6z",
    plus: "M12 5v14 M5 12h14",
    settings: "M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6z M19 12l2-1-1-3-2 .2-1.3-1.3.2-2-3-1-1 2h-1.8l-1-2-3 1 .2 2L7 8.2 5 8l-1 3 2 1v1.8l-2 1 1 3 2-.2 1.3 1.3-.2 2 3 1 1-2h1.8l1 2 3-1-.2-2 1.3-1.3 2 .2 1-3-2-1z",
  };
</script>

<svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"
  stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
  <path d={paths[name] ?? paths.folder} />
</svg>
