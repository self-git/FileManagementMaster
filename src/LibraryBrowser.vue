<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import DesktopIcon from './DesktopIcon.vue'
import { fileName, fileSuffix, type Book, type BrowserSelection, type Library, type WorkspaceModule } from './library-types'

const props = defineProps<{
  library: Library
  module: WorkspaceModule
  folder: string
  query: string
  selection: BrowserSelection | null
}>()
const emit = defineEmits<{
  select: [selection: BrowserSelection | null]
  enterFolder: [path: string]
  openFile: [path: string]
  read: []
}>()

type Row =
  | { kind: 'folder'; path: string; name: string; key: string }
  | { kind: 'file'; path: string; name: string; key: string; nested: boolean }
  | { kind: 'book'; book: Book; name: string; key: string }

const collator = new Intl.Collator('zh-CN', { numeric: true, sensitivity: 'base' })
const list = ref<HTMLElement | null>(null)
const focusedKey = ref('')
// 连选锚点来自当前列表，目录、分支和搜索条件改变时重置。
const anchor = ref('')
const expanded = ref(new Set<number>())
const query = computed(() => props.query.trim().toLocaleLowerCase())
const sortByName = (left: Row, right: Row) => collator.compare(left.name, right.name)
const fileRow = (path: string, nested = false): Row => ({
  kind: 'file', path, name: fileName(path), key: `file:${path}`, nested,
})

const fileRows = computed(() => {
  const prefix = props.folder ? `${props.folder}/` : ''
  const folders = new Map<string, Row>()
  const files: Row[] = []
  for (const path of props.library.files) {
    if (!path.startsWith(prefix)) continue
    const relative = path.slice(prefix.length)
    const slash = relative.indexOf('/')
    if (query.value) {
      if (relative.toLocaleLowerCase().includes(query.value)) files.push(fileRow(path))
    } else if (slash >= 0) {
      const name = relative.slice(0, slash)
      const folder = prefix + name
      folders.set(folder, { kind: 'folder', path: folder, name, key: `folder:${folder}` })
    } else {
      files.push(fileRow(path))
    }
  }
  return [...Array.from(folders.values()).sort(sortByName), ...files.sort(sortByName)]
})

function matches(...values: (string | null | undefined)[]) {
  return values.some(value => value?.toLocaleLowerCase().includes(query.value))
}

const bookRows = computed(() => {
  const grouped = new Set(props.library.books.flatMap(book => book.files))
  const rows: Row[] = []
  const books = [...props.library.books].sort((a, b) =>
    collator.compare(a.title, b.title) || collator.compare(a.author ?? '', b.author ?? ''),
  )
  for (const book of books) {
    const matchingBook = !query.value || matches(book.title, book.author)
    const matchingFiles = matchingBook ? book.files : book.files.filter(path => matches(path))
    if (!matchingFiles.length) continue
    rows.push({ kind: 'book', book, name: book.title, key: `book:${book.id}` })
    if (query.value || expanded.value.has(book.id)) {
      rows.push(...matchingFiles.map(path => fileRow(path, true)).sort(sortByName))
    }
  }
  const ungrouped = props.library.files.filter(path => {
    const novel = props.library.novels[path]
    return !grouped.has(path) && !!novel?.format && (!query.value || matches(path, novel.title, novel.author))
  }).map(path => fileRow(path)).sort(sortByName)
  return [...rows, ...ungrouped]
})

const rows = computed(() => props.module === 'books' ? bookRows.value : fileRows.value)
const tabKey = computed(() => rows.value.some(row => row.key === focusedKey.value) ? focusedKey.value : rows.value[0]?.key)
const selectionPaths = computed(() => new Set(props.selection?.kind === 'files' ? props.selection.paths : []))

watch([() => props.library.directory, () => props.library.currentBranch, () => props.module], () => {
  expanded.value.clear()
})
watch([() => props.folder, () => props.module, () => props.query, () => props.library.currentBranch], () => {
  anchor.value = ''
  focusedKey.value = ''
})

function chosen(row: Row) {
  const selection = props.selection
  if (row.kind === 'file') return selectionPaths.value.has(row.path)
    || (row.nested && selection?.kind === 'book' && props.library.books.find(book => book.id === selection.id)?.files.includes(row.path)) || false
  if (row.kind === 'book') return selection?.kind === 'book' && selection.id === row.book.id
  return selection?.kind === 'folder' && selection.path === row.path
}

function select(row: Row, modifiers: Pick<MouseEvent, 'shiftKey' | 'metaKey' | 'ctrlKey'>) {
  focusedKey.value = row.key
  if (row.kind === 'book') {
    anchor.value = ''
    emit('select', { kind: 'book', id: row.book.id })
    return
  }
  if (row.kind === 'folder') {
    anchor.value = ''
    emit('select', { kind: 'folder', path: row.path })
    return
  }
  const toggle = modifiers.metaKey || modifiers.ctrlKey
  const paths = toggle ? new Set(selectionPaths.value) : new Set<string>()
  const index = modifiers.shiftKey ? rows.value.findIndex(item => item.key === anchor.value) : -1
  if (index >= 0) {
    const target = rows.value.indexOf(row)
    for (const item of rows.value.slice(Math.min(index, target), Math.max(index, target) + 1)) {
      if (item.kind === 'file') paths.add(item.path)
    }
  } else {
    anchor.value = row.key
    if (toggle && paths.has(row.path)) paths.delete(row.path)
    else paths.add(row.path)
  }
  emit('select', paths.size ? { kind: 'files', paths: [...paths] } : null)
}

function disclose(id: number) {
  if (!expanded.value.delete(id)) expanded.value.add(id)
}

function activate(row: Row) {
  if (row.kind === 'folder') emit('enterFolder', row.path)
  else if (row.kind === 'book') disclose(row.book.id)
  else emit('openFile', row.path)
}

function focus(key = tabKey.value) {
  if (!key) {
    list.value?.focus()
    return
  }
  const controls = list.value?.querySelectorAll<HTMLElement>('[data-row-key]')
  const target = controls && Array.from(controls).find(element => element.dataset.rowKey === key)
  target?.focus()
}

defineExpose({ focus })

function onRowKey(event: KeyboardEvent, row: Row) {
  if (event.key === 'Enter') {
    event.preventDefault()
    activate(row)
  } else if (row.kind === 'book' && ['ArrowRight', 'ArrowLeft'].includes(event.key)) {
    event.preventDefault()
    if (event.key === 'ArrowRight') expanded.value.add(row.book.id)
    else expanded.value.delete(row.book.id)
  } else if (['ArrowDown', 'ArrowUp'].includes(event.key)) {
    event.preventDefault()
    const next = rows.value[rows.value.indexOf(row) + (event.key === 'ArrowDown' ? 1 : -1)]
    if (!next) return
    if (event.shiftKey || (!event.metaKey && !event.ctrlKey)) select(next, event)
    void nextTick(() => focus(next.key))
  }
}

function formats(row: Row) {
  const paths = row.kind === 'book' ? row.book.files : row.kind === 'file' ? [row.path] : []
  return [...new Set(paths.map(fileSuffix).filter(Boolean))].map(format => format.toUpperCase())
}

function progress(row: Row) {
  const paths = row.kind === 'book' ? row.book.files : row.kind === 'file' ? [row.path] : []
  return [...new Set(paths.map(path => {
    const novel = props.library.novels[path]
    return novel?.completed ? '完结' : novel?.progress
  }).filter(Boolean))].join(' · ')
}
</script>

<template>
  <div ref="list" class="browser-list" :class="{ 'book-list': module === 'books' }" tabindex="-1">
    <template v-if="rows.length">
      <div class="list-columns list-header" aria-hidden="true">
        <span>{{ module === 'books' ? '书目 / 文件' : '名称' }}</span>
        <template v-if="module === 'books'"><span class="author-column">作者</span><span>格式</span><span>进度</span></template>
        <span v-else>种类</span>
      </div>
      <ul class="entry-list" :aria-label="module === 'books' ? '书目和文件' : '目录文件'">
        <li v-for="row in rows" :key="row.key" :class="{ 'book-entry': row.kind === 'book', 'book-child': row.kind === 'file' && row.nested }">
          <button class="list-columns entry-row" type="button" :data-row-key="row.key" :tabindex="row.key === tabKey ? 0 : -1"
            :aria-pressed="chosen(row)" @focus="focusedKey = row.key" @click="select(row, $event)"
            @dblclick="activate(row)" @keydown="onRowKey($event, row)">
            <span class="entry-name">
              <DesktopIcon v-if="row.kind === 'folder'" name="folder" class="folder-glyph" />
              <span v-else-if="row.kind === 'book'" class="book-glyph"><DesktopIcon name="book" /></span>
              <span v-else class="file-glyph">{{ fileSuffix(row.path).toUpperCase().slice(0, 4) }}</span>
              <span class="entry-title">
                <span class="truncate" :title="row.name">{{ row.name }}</span>
                <small v-if="row.kind === 'file' && (row.nested || (query && row.path.includes('/')))" class="truncate" :title="row.path">{{ row.kind === 'file' && row.nested ? row.path.slice(0, row.path.lastIndexOf('/')) : row.path }}</small>
              </span>
            </span>
            <template v-if="module === 'books'">
              <span class="cell-meta author-column truncate">{{ row.kind === 'book' ? row.book.author : row.kind === 'file' && !row.nested ? library.novels[row.path]?.author : '' }}</span>
              <span class="format-group" :title="formats(row).join('、')"><span v-for="format in formats(row)" :key="format" class="format-tag">{{ format }}</span></span>
              <span class="cell-meta truncate" :title="progress(row)">{{ progress(row) || '—' }}</span>
            </template>
            <span v-else class="cell-meta truncate">{{ row.kind === 'folder' ? '文件夹' : row.kind === 'file' ? (fileSuffix(row.path).toUpperCase() || '文件') : '' }}</span>
          </button>
          <button v-if="row.kind === 'book'" class="icon-button disclose" type="button" :aria-expanded="!!query || expanded.has(row.book.id)"
            :aria-label="`${query || expanded.has(row.book.id) ? '收起' : '展开'} ${row.book.title}`" :disabled="!!query" @click="disclose(row.book.id)">
            <DesktopIcon :name="query || expanded.has(row.book.id) ? 'down' : 'right'" />
          </button>
        </li>
      </ul>
    </template>
    <div v-else class="empty-state">
      <DesktopIcon :name="query ? 'search' : module === 'books' ? 'books' : 'folder'" />
      <h2>{{ query ? '没有匹配的项目' : module === 'books' ? '书库暂时为空' : '此目录暂无文件' }}</h2>
      <p v-if="query">请试试其他名称或关键字。</p>
      <button v-else class="button" type="button" @click="$emit('read')">{{ module === 'books' ? '扫描小说' : '读取来源目录' }}</button>
    </div>
  </div>
</template>
