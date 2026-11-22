<script setup lang="ts">
const paths = {
  sidebar: 'M3 4h18v16H3zM9 4v16',
  folder: 'M3 7a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z',
  open: 'M3 16V7a2 2 0 0 1 2-2h5l2 2h6a2 2 0 0 1 2 2v1M3 16l2-6h17l-3 9H5a2 2 0 0 1-2-2Z',
  books: 'M12 5a6 6 0 0 0-4-2H3v15h5a6 6 0 0 1 4 2 6 6 0 0 1 4-2h5V3h-5a6 6 0 0 0-4 2Zm0 0v15',
  book: 'M5 4a2 2 0 0 1 2-2h12v19H7a2 2 0 0 1-2-2Zm0 14a2 2 0 0 1 2-2h12M9 6h6',
  file: 'M6 3h8l5 5v13H6zM14 3v6h5M9 13h7M9 17h5',
  branch: 'M6 7v10m0-4h6a5 5 0 0 0 5-5v-1M8 5a2 2 0 1 1-4 0 2 2 0 0 1 4 0Zm11 0a2 2 0 1 1-4 0 2 2 0 0 1 4 0ZM8 19a2 2 0 1 1-4 0 2 2 0 0 1 4 0Z',
  search: 'M19 19l-4-4m2-4a6 6 0 1 1-12 0 6 6 0 0 1 12 0Z',
  info: 'M22 12a10 10 0 1 1-20 0 10 10 0 0 1 20 0Zm-10 4v-4m0-4h.01',
  more: 'M5 12h.01M12 12h.01M19 12h.01',
  plus: 'M12 5v14M5 12h14',
  close: 'M6 6l12 12M6 18 18 6',
  right: 'm9 5 7 7-7 7',
  down: 'm5 9 7 7 7-7',
  back: 'm14 5-7 7 7 7',
  import: 'M12 3v12m-5-5 5 5 5-5M4 15v5h16v-5',
  scan: 'M9 4H4v5m11-5h5v5M4 15v5h5m11-5v5h-5M4 12h16',
  history: 'M4 11a8 8 0 1 1 2 6m-2-6V6m0 5h5m3-4v5l3 2',
  move: 'M3 12h17m-6-6 6 6-6 6',
  external: 'M13 4h7v7m0-7-9 9m8 0v7H4V5h7',
  copy: 'M8 8h12v12H8zM16 8V4H4v12h4',
  check: 'm5 12 4 4L19 6',
  warning: 'M12 3 2 21h20L12 3Zm0 7v4m0 3h.01',
  storage: 'M4 11h16l2 5v4H2v-4zm0 5h16m-3 2h.01m2 0h.01',
  settings: 'M12 15a3 3 0 1 1 0-6 3 3 0 0 1 0 6ZM12 2l2 3 4-.2 1.2 3.3L22 11v2l-2.8 2.9-1.2 3.3-4-.2-2 3-2-3-4 .2-1.2-3.3L2 13v-2l2.8-2.9L6 4.8l4 .2Z',
} as const

defineProps<{ name: keyof typeof paths }>()
</script>

<template>
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.65"
    stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false">
    <path :d="paths[name]" :stroke-width="name === 'more' ? 3 : undefined" />
  </svg>
</template>
