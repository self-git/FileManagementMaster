<script setup lang="ts">
import { nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'

const props = defineProps<{ title: string; busy: boolean; wide?: boolean }>()
const emit = defineEmits<{ close: []; returnFocus: [] }>()
const dialog = ref<HTMLElement | null>(null)
let previousFocus: HTMLElement | null = null

function focusSheet() {
  const target = dialog.value?.querySelector<HTMLElement>('[data-initial-focus]') ?? dialog.value
  target?.focus()
}

function onKey(event: KeyboardEvent) {
  if (event.key === 'Escape') {
    event.preventDefault()
    event.stopImmediatePropagation()
    if (!props.busy) emit('close')
  }
  if (event.key !== 'Tab') return
  const controls = Array.from(dialog.value?.querySelectorAll<HTMLElement>(
    'button:not([disabled]), input:not([disabled]), select:not([disabled]), [tabindex="0"]',
  ) ?? []).filter(element => element.getClientRects().length > 0)
  const target = event.shiftKey ? controls.at(-1) : controls[0]
  const boundary = event.shiftKey ? controls[0] : controls.at(-1)
  if (!target || document.activeElement === boundary || !controls.some(item => item === document.activeElement)) {
    event.preventDefault()
    const next = target ?? dialog.value
    next?.focus()
  }
}

onMounted(() => {
  if (document.activeElement instanceof HTMLElement) previousFocus = document.activeElement
  window.addEventListener('keydown', onKey, true)
  focusSheet()
})
watch(() => props.title, () => nextTick(focusSheet))
onBeforeUnmount(() => {
  window.removeEventListener('keydown', onKey, true)
  void nextTick(() => {
    if (previousFocus?.isConnected && previousFocus !== document.body && !previousFocus.matches(':disabled')) {
      previousFocus.focus()
    } else {
      emit('returnFocus')
    }
  })
})
</script>

<template>
  <div class="sheet-overlay">
    <section ref="dialog" class="desktop-sheet" :class="{ 'sheet-wide': wide }" tabindex="-1"
      role="dialog" aria-modal="true" aria-labelledby="sheet-title" :aria-busy="busy">
      <header class="sheet-heading"><h2 id="sheet-title">{{ title }}</h2></header>
      <slot />
    </section>
  </div>
</template>
