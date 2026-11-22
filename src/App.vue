<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { computed, nextTick, onBeforeUnmount, onMounted, ref, shallowRef, watch } from 'vue'
import DesktopIcon from './DesktopIcon.vue'
import DesktopSheet from './DesktopSheet.vue'
import EngineVerification from './EngineVerification.vue'
import LibraryBrowser from './LibraryBrowser.vue'
import LibraryReading from './LibraryReading.vue'
import { changeLabels, fileName, fileSuffix, type Book, type BrowserSelection, type ExternalChange, type Library, type WorkspaceModule } from './library-types'
import './library.css'

interface SwitchPreview {
  sourceBranch: string
  targetBranch: string
  token: string
  changes: ExternalChange[]
}

interface LibraryTransition {
  library: Library
  pending: SwitchPreview | null
  recovery: 'beforeCommit' | 'afterCommit' | null
}

type MoveSelection = { kind: 'book'; id: number } | { kind: 'files'; paths: string[] }
type MoveConfirmation =
  | { kind: 'saveExternal'; token: string }
  | { kind: 'move'; token: string; destinations: Record<string, string> }
type MoveStatus =
  | { kind: 'saveExternal'; token: string; changes: ExternalChange[] }
  | { kind: 'preview'; token: string; items: { path: string; conflict: string | null }[] }
  | { kind: 'savedExternal' }
  | { kind: 'moved' }

interface MoveIntent {
  sourceBranch: string
  targetBranch: string
  selection: MoveSelection
}

interface MoveTransition {
  library: Library
  status: MoveStatus
}

interface OldVersion {
  order: number
  savedPath: string
  destination: string
}

interface FileHistory {
  token: string
  current: { branch: string; path: string }[]
  versions: OldVersion[]
}

interface CleanupReport {
  beforeBytes: number
  afterBytes: number
}

type Panel = 'chooser' | 'branch' | 'switch' | 'move' | 'reading' | 'history' | 'storage' | 'diagnostics' | 'book'
type Visibility = 'auto' | 'shown' | 'hidden'

const modules = { files: '文件管理', books: '书库管理' } satisfies Record<WorkspaceModule, string>
const moduleKey = 'filemanagement.active-module'
const isMac = /Mac/.test(navigator.platform)
const revealLabel = isMac ? '在 Finder 中显示' : '在文件资源管理器中显示'
const shortcut = isMac ? '⌘' : 'Ctrl+'
const activeModule = ref<WorkspaceModule>(localStorage.getItem(moduleKey) === 'books' ? 'books' : 'files')

// 两个模块共用 Rust 返回的当前目录、HEAD 和已保存文件清单。
const library = shallowRef<Library | null>(null)
const folder = ref('')
const query = ref('')
const selection = shallowRef<BrowserSelection | null>(null)
const panel = ref<Panel | null>(null)
const activeMenu = ref<{ kind: 'toolbar' } | { kind: 'branch'; branch: string } | null>(null)
const sidebar = ref<Visibility>('auto')
const inspector = ref<Visibility>('auto')
const browser = ref<InstanceType<typeof LibraryBrowser> | null>(null)
const searchInput = ref<HTMLInputElement | null>(null)
const openButton = ref<HTMLButtonElement | null>(null)
let menuTrigger: HTMLElement | null = null

const task = ref('')
const busy = computed(() => !!task.value)
const error = ref('')
const notice = ref('')
const recoveryNotice = ref('')
let noticeTimer: number | undefined

// 目录来源来自系统选择器，独立纳管以该目录作为父目录。
const chosenDirectory = ref('')
const createMode = ref(false)
const directoryName = ref('')
const branchName = ref('')
const bookTitle = ref('')
const readingEntry = ref<'scan' | 'import' | 'save'>('scan')

// 确认时回传最初的操作意图和 Rust 预览票据。
const pending = shallowRef<SwitchPreview | null>(null)
const moveTarget = ref('')
const movePending = shallowRef<{
  intent: MoveIntent
  status: Extract<MoveStatus, { kind: 'saveExternal' | 'preview' }>
} | null>(null)
const newDestinations = ref<Record<string, string>>({})
const history = shallowRef<{ path: string; book: Book | null; data: FileHistory } | null>(null)
const restoreChoice = shallowRef<OldVersion | null>(null)

const cleanupNotice = ref('')
const collecting = ref(false)
let cleanupTimer: number | undefined
let cleanupQueued = false
let cleanupInFlight: Promise<void> | null = null
let unlistenDiagnostics: UnlistenFn | undefined

const currentBranch = computed(() => library.value?.currentBranch ?? '')
const otherBranches = computed(() => library.value?.branches.filter(name => name !== currentBranch.value) ?? [])
const canEdit = computed(() => !busy.value && !panel.value)
const pathLabel = computed(() => {
  const path = library.value?.directory ?? ''
  if (path.startsWith('\\\\?\\UNC\\')) return '\\\\' + path.slice(8)
  return path.startsWith('\\\\?\\') ? path.slice(4) : path
})
const libraryName = computed(() => pathLabel.value.split(/[\\/]/).filter(Boolean).at(-1) ?? '文件管理大师')
const crumbs = computed(() => folder.value ? folder.value.split('/').map((name, i, parts) => ({
  name, path: parts.slice(0, i + 1).join('/'),
})) : [])
const pickedBook = computed(() => {
  const picked = selection.value
  return picked?.kind === 'book' ? library.value?.books.find(book => book.id === picked.id) : undefined
})
const selectedFiles = computed(() => selection.value?.kind === 'files' ? selection.value.paths : [])
const singleFile = computed(() => selectedFiles.value.length === 1 ? selectedFiles.value[0] : undefined)
const selectedFolder = computed(() => selection.value?.kind === 'folder' ? selection.value.path : undefined)
const pickedPath = computed(() => singleFile.value ?? selectedFolder.value)
const canMove = computed(() => !!(pickedBook.value || selectedFiles.value.length) && otherBranches.value.length > 0)
const canGroup = computed(() => {
  if (activeModule.value !== 'books' || !selectedFiles.value.length) return false
  const grouped = new Set(library.value?.books.flatMap(book => book.files))
  return selectedFiles.value.every(file => !grouped.has(file))
})
const contextBook = computed(() => pickedBook.value
  ?? (activeModule.value === 'books' && singleFile.value
    ? library.value?.books.find(book => book.files.includes(singleFile.value ?? ''))
    : undefined))
const visibleFileCount = computed(() => {
  const prefix = folder.value ? `${folder.value}/` : ''
  return library.value?.files.filter(path => path.startsWith(prefix)).length ?? 0
})
const bookCount = computed(() => library.value?.books.length ?? 0)
const selectedCount = computed(() => pickedBook.value?.files.length ?? (selectedFolder.value ? 1 : selectedFiles.value.length))
const historyTabs = computed(() => {
  const active = history.value
  if (!active?.book) return []
  const formats = new Map<string, { path: string; label: string }>()
  for (const path of active.book.files) {
    const format = library.value?.novels[path]?.format
    const key = format ?? path
    if (!formats.has(key) || path === active.path) {
      formats.set(key, { path, label: format?.toUpperCase() ?? fileName(path) })
    }
  }
  return [...formats.values()]
})
const title = computed(() => {
  switch (panel.value) {
    case 'chooser': return '打开管理目录'
    case 'branch': return '新建分支'
    case 'switch': return '先保存外部改动'
    case 'move': return movePending.value?.status.kind === 'saveExternal' ? '先保存外部改动' : '移至分支'
    case 'reading': return readingEntry.value === 'save' ? '保存外部改动'
      : readingEntry.value === 'scan' ? '扫描小说' : activeModule.value === 'books' ? '导入更新' : '读取来源目录'
    case 'history': return restoreChoice.value ? '恢复旧版' : '版本历史'
    case 'book': return '创建书目'
    case 'storage': return '空间管理'
    case 'diagnostics': return '诊断'
    default: return ''
  }
})

function notify(message: string) {
  window.clearTimeout(noticeTimer)
  notice.value = message
  noticeTimer = window.setTimeout(() => { notice.value = '' }, 5200)
}

async function run(operation: () => Promise<void>, label = '正在处理…') {
  if (busy.value) return
  task.value = label
  error.value = ''
  try {
    if (cleanupInFlight) await cleanupInFlight
    await operation()
  } catch (reason) {
    error.value = reason instanceof Error ? reason.message : String(reason)
  } finally {
    task.value = ''
    queueCleanup()
  }
}

function runReading(operation: () => Promise<void>) {
  return run(operation, '正在读取目录或保存文件…')
}

watch([() => library.value?.directory, () => library.value?.revision], () => {
  cleanupQueued = true
  queueCleanup()
})
watch(panel, queueCleanup)

function queueCleanup() {
  window.clearTimeout(cleanupTimer)
  if (!cleanupQueued || busy.value || panel.value || cleanupInFlight || !library.value) return
  // 合并短时间内的保存。界面写操作等当前回收进程结束再执行。
  cleanupTimer = window.setTimeout(() => {
    const directory = library.value?.directory
    if (!directory || busy.value || panel.value || cleanupInFlight) return
    cleanupQueued = false
    cleanupInFlight = collect(directory, false).finally(() => {
      cleanupInFlight = null
      queueCleanup()
    })
  }, 1600)
}

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`
  const unit = bytes >= 1024 ** 3 ? 3 : bytes >= 1024 ** 2 ? 2 : 1
  return `${(bytes / 1024 ** unit).toLocaleString('zh-CN', { maximumFractionDigits: 1 })} ${['', 'KiB', 'MiB', 'GiB'][unit]}`
}

async function collect(directory: string, manual: boolean) {
  collecting.value = true
  try {
    const report = await invoke<CleanupReport | null>('collect_garbage', { directory })
    if (library.value?.directory !== directory) return
    if (report) {
      const released = report.beforeBytes - report.afterBytes
      cleanupNotice.value = released > 0
        ? `对象库已释放 ${formatBytes(released)}`
        : `回收已完成，对象库占用 ${formatBytes(report.afterBytes)}`
      if (!manual) notify(cleanupNotice.value)
    } else if (manual && !cleanupNotice.value) {
      cleanupNotice.value = '当前没有可执行的回收。'
    }
  } catch (reason) {
    if (library.value?.directory === directory) cleanupNotice.value = `回收等待处理。${String(reason)}`
  } finally {
    collecting.value = false
  }
}

function cleanupNow() {
  const directory = library.value?.directory
  if (!directory) return
  return run(async () => {
    cleanupQueued = false
    await collect(directory, true)
  }, '正在回收空间…')
}

function updateLibrary(updated: Library) {
  const previous = library.value
  const files = new Set(updated.files)
  if (previous?.directory !== updated.directory) {
    recoveryNotice.value = ''
    cleanupNotice.value = ''
  }
  if (previous?.directory !== updated.directory || previous.currentBranch !== updated.currentBranch) {
    selection.value = null
    folder.value = ''
    query.value = ''
  } else {
    if (selection.value?.kind === 'files') {
      const paths = selection.value.paths.filter(path => files.has(path))
      selection.value = paths.length ? { kind: 'files', paths } : null
    } else if (selection.value?.kind === 'book' && !updated.books.some(book => book.id === pickedBook.value?.id)) {
      selection.value = null
    } else if (selection.value?.kind === 'folder') {
      const prefix = `${selection.value.path}/`
      if (!updated.files.some(path => path.startsWith(prefix))) selection.value = null
    }
    while (folder.value && !updated.files.some(path => path.startsWith(`${folder.value}/`))) {
      folder.value = folder.value.slice(0, Math.max(0, folder.value.lastIndexOf('/')))
    }
  }
  library.value = updated
  localStorage.setItem(moduleKey, activeModule.value)
}

function applyTransition(result: LibraryTransition) {
  updateLibrary(result.library)
  if (result.recovery) {
    recoveryNotice.value = result.recovery === 'beforeCommit'
      ? '已恢复上次操作中暂存的原文件。'
      : '上次提交已生效，已完成工作目录和索引恢复。'
  }
  pending.value = result.pending
  if (result.pending) panel.value = 'switch'
  else if (panel.value === 'switch' || panel.value === 'chooser') closePanel()
}

function selectModule(module: WorkspaceModule) {
  if (module !== activeModule.value) {
    activeModule.value = module
    selection.value = null
    query.value = ''
    if (library.value) localStorage.setItem(moduleKey, module)
  }
  if (window.matchMedia('(max-width: 640px)').matches) sidebar.value = 'hidden'
}

function openPanel(kind: Panel) {
  if (!canEdit.value) return
  error.value = ''
  activeMenu.value = null
  panel.value = kind
}

function closePanel() {
  panel.value = null
  pending.value = null
  movePending.value = null
  moveTarget.value = ''
  newDestinations.value = {}
  history.value = null
  restoreChoice.value = null
  error.value = ''
}

function chooseDirectory() {
  return run(async () => {
    const directory = await invoke<string | null>('choose_library_directory', { title: '选择管理目录' })
    if (directory) chosenDirectory.value = directory
  }, '正在选择目录…')
}

function openLibrary() {
  const directory = chosenDirectory.value
  if (!directory) return
  return run(async () => {
    const result = createMode.value
      ? await invoke<LibraryTransition>('create_library', { parent: directory, name: directoryName.value.trim() })
      : await invoke<LibraryTransition>('adopt_library', { directory })
    applyTransition(result)
    chosenDirectory.value = ''
    directoryName.value = ''
    createMode.value = false
  }, createMode.value ? '正在创建管理目录…' : '正在打开管理目录…')
}

function showChooser() {
  chosenDirectory.value = ''
  directoryName.value = ''
  createMode.value = false
  openPanel('chooser')
}

function createBranch() {
  const directory = library.value?.directory
  const name = branchName.value.trim()
  if (!directory || !name) return
  return run(async () => {
    updateLibrary(await invoke<Library>('create_branch', { directory, name }))
    closePanel()
    notify(`已创建分支「${name}」。`)
  }, '正在创建分支…')
}

function showBranchEditor() {
  branchName.value = ''
  openPanel('branch')
}

function setDefault(name: string) {
  const directory = library.value?.directory
  if (!directory || !canEdit.value) return
  closeMenu(true)
  return run(async () => {
    updateLibrary(await invoke<Library>('set_default_branch', { directory, name }))
    notify(`已将「${name}」设为默认分支。`)
  }, '正在设置默认分支…')
}

function switchBranch(name: string, token?: string) {
  const directory = library.value?.directory
  if (!directory) return
  return run(async () => {
    const source = currentBranch.value
    // 确认票据过期时，保留原分支并使用 Rust 返回的新预览。
    const result = await invoke<LibraryTransition>('switch_branch', { directory, name, token })
    applyTransition(result)
    if (token && result.pending) error.value = '文件又发生了变动，请核对新的改动清单。'
    if (source !== result.library.currentBranch) {
      if (window.matchMedia('(max-width: 640px)').matches) sidebar.value = 'hidden'
      notify(`已切换至「${result.library.currentBranch}」。`)
    }
  }, '正在检查并切换分支…')
}

function navigateFolder(path: string) {
  folder.value = path
  selection.value = null
  query.value = ''
  void nextTick(() => browser.value?.focus())
}

function createBook() {
  const current = library.value
  const title = bookTitle.value.trim()
  const paths = selectedFiles.value
  if (!current || !canGroup.value || !title) return
  return run(async () => {
    const updated = await invoke<Library>('create_book', {
      directory: current.directory, sourceBranch: current.currentBranch, title, paths,
    })
    updateLibrary(updated)
    const book = updated.books.find(item => item.files.includes(paths[0]))
    if (book) selection.value = { kind: 'book', id: book.id }
    closePanel()
    notify(`已创建书目「${title}」。`)
  }, '正在创建书目…')
}

function showCreateBook() {
  bookTitle.value = ''
  openPanel('book')
}

function showReading(entry: 'scan' | 'import' | 'save') {
  readingEntry.value = entry
  openPanel('reading')
}

function completeReading(message: string) {
  closePanel()
  notify(message)
}

function showMove() {
  if (!canMove.value) return
  moveTarget.value = otherBranches.value.length === 1 ? otherBranches.value[0] : ''
  openPanel('move')
}

function startMove() {
  if (!library.value || !moveTarget.value || !canMove.value) return
  return requestMove({
    sourceBranch: currentBranch.value,
    targetBranch: moveTarget.value,
    selection: pickedBook.value
      ? { kind: 'book', id: pickedBook.value.id }
      : { kind: 'files', paths: [...selectedFiles.value] },
  })
}

function changeMoveTarget() {
  movePending.value = null
  newDestinations.value = {}
  error.value = ''
}

function destinationError(path: string) {
  const value = newDestinations.value[path] ?? ''
  if (!value.trim()) return '请填写新目标相对路径'
  if (value.includes('\\') || value.split('/').some(part => !part || part === '.' || part === '..' || part.toLowerCase() === '.git')) {
    return '请输入有效的相对路径'
  }
  if (fileSuffix(path) !== fileSuffix(value)) return '请保留原文件后缀'
  return ''
}

function suggestDestination(path: string) {
  const name = fileName(path)
  const suffix = fileSuffix(name)
  const split = suffix ? name.length - suffix.length - 1 : name.length
  newDestinations.value[path] = path.slice(0, path.length - name.length) + name.slice(0, split) + '（移入）' + name.slice(split)
}

const canConfirmMove = computed(() => {
  const status = movePending.value?.status
  return status?.kind === 'saveExternal' || (status?.kind === 'preview'
    && status.items.every(item => !item.conflict || !destinationError(item.path)))
})

function confirmMove() {
  const preview = movePending.value
  if (!preview || !canConfirmMove.value) return
  const { status, intent } = preview
  const confirmation: MoveConfirmation = status.kind === 'saveExternal'
    ? { kind: 'saveExternal', token: status.token }
    : { kind: 'move', token: status.token, destinations: { ...newDestinations.value } }
  return requestMove(intent, confirmation)
}

function requestMove(intent: MoveIntent, confirmation?: MoveConfirmation) {
  const directory = library.value?.directory
  if (!directory) return
  const count = movePending.value?.status.kind === 'preview' ? movePending.value.status.items.length : selectedCount.value
  return run(async () => {
    const request = (approval?: MoveConfirmation) => invoke<MoveTransition>('move_files', {
      directory, request: { ...intent, confirmation: approval },
    })
    let result = await request(confirmation)
    updateLibrary(result.library)
    if (result.status.kind === 'savedExternal') {
      const selected = intent.selection
      const stillAvailable = selected.kind === 'book'
        ? result.library.books.some(book => book.id === selected.id)
        : selected.paths.every(path => result.library.files.includes(path))
      if (!stillAvailable) {
        closePanel()
        notify('外部改动已保存，请重新选择移动项。')
        return
      }
      movePending.value = null
      result = await request()
      updateLibrary(result.library)
    }
    const { status } = result
    if (status.kind === 'preview' || status.kind === 'saveExternal') {
      movePending.value = { intent, status }
      newDestinations.value = status.kind === 'preview'
        ? Object.fromEntries(status.items.filter(item => item.conflict).map(item => [item.path, '']))
        : {}
      if ((confirmation?.kind === 'move') || (confirmation?.kind === 'saveExternal' && status.kind === 'saveExternal')) {
        error.value = '目录或分支有新变化，请重新核对预览。'
      }
    } else if (status.kind === 'moved') {
      selection.value = null
      closePanel()
      notify(`已将 ${count} 项移至「${intent.targetBranch}」，当前仍为「${intent.sourceBranch}」。`)
    }
  }, confirmation?.kind === 'move' ? '正在移动并提交文件…' : '正在检查移动清单…')
}

function showHistory(path: string, book: Book | null = contextBook.value ?? null) {
  const current = library.value
  if (!current || busy.value) return
  return run(async () => {
    const data = await invoke<FileHistory>('file_history', {
      directory: current.directory, branch: current.currentBranch, path,
    })
    history.value = { path, book, data }
    restoreChoice.value = null
    panel.value = 'history'
  }, '正在读取版本历史…')
}

function restoreSelectedVersion() {
  const current = library.value
  const active = history.value
  const version = restoreChoice.value
  if (!current || !active || !version) return
  return run(async () => {
    const updated = await invoke<Library>('restore_version', {
      directory: current.directory,
      request: { branch: current.currentBranch, path: active.path, token: active.data.token, order: version.order },
    })
    updateLibrary(updated)
    selection.value = { kind: 'files', paths: [version.destination] }
    closePanel()
    notify('已恢复所选旧版。')
  }, '正在恢复旧版…')
}

function openPath(relative?: string, reveal = false) {
  const current = library.value
  if (!current) return
  return run(() => invoke<void>('open_library_path', {
    directory: current.directory, branch: current.currentBranch, relative, reveal,
  }), '正在打开实际路径…')
}

function absolutePath(relative = '') {
  const root = pathLabel.value
  if (!relative) return root
  if (isMac) return root.replace(/\/$/, '') + '/' + relative
  return root.replace(/\\$/, '') + '\\' + relative.replaceAll('/', '\\')
}

async function copyPath(relative?: string) {
  const path = absolutePath(relative)
  if (!path) return
  closeMenu()
  await run(async () => {
    await navigator.clipboard.writeText(path)
    notify('已复制路径。')
  })
}

function showMenu(menu: NonNullable<typeof activeMenu.value>, event: MouseEvent) {
  if (activeMenu.value?.kind === menu.kind &&
    (menu.kind === 'toolbar' || (activeMenu.value.kind === 'branch' && activeMenu.value.branch === menu.branch))) {
    closeMenu(true)
    return
  }
  if (event.currentTarget instanceof HTMLElement) menuTrigger = event.currentTarget
  activeMenu.value = menu
  void nextTick(() => document.querySelector<HTMLElement>('.action-menu button:not([disabled])')?.focus())
}

function closeMenu(focus = false) {
  if (focus && activeMenu.value) menuTrigger?.focus()
  activeMenu.value = null
  menuTrigger = null
}

function onMenuKey(event: KeyboardEvent) {
  if (!['ArrowUp', 'ArrowDown'].includes(event.key) || !(event.currentTarget instanceof HTMLElement)) return
  const buttons = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>('button:not([disabled])'))
  if (!buttons.length) return
  event.preventDefault()
  const index = buttons.findIndex(button => button === document.activeElement)
  buttons[(index + buttons.length + (event.key === 'ArrowDown' ? 1 : -1)) % buttons.length].focus()
}

function toggleSidebar() {
  const shown = sidebar.value === 'shown' || (sidebar.value === 'auto' && !window.matchMedia('(max-width: 640px)').matches)
  sidebar.value = shown ? 'hidden' : 'shown'
}

function toggleInspector() {
  const shown = inspector.value === 'shown'
    || (inspector.value === 'auto' && !!selection.value && !window.matchMedia('(max-width: 760px)').matches)
  inspector.value = shown ? 'hidden' : 'shown'
}

function returnFocus() {
  if (browser.value) browser.value.focus()
  else openButton.value?.focus()
}

function onShortcut(event: KeyboardEvent) {
  if (panel.value || event.defaultPrevented) return
  if (event.key === 'Escape') {
    if (activeMenu.value) closeMenu(true)
    else if (query.value) query.value = ''
    else if (window.matchMedia('(max-width: 760px)').matches && inspector.value === 'shown') inspector.value = 'hidden'
    return
  }
  if ((!event.metaKey && !event.ctrlKey) || event.altKey) return
  if (event.key.toLowerCase() === 'o') {
    event.preventDefault()
    if (canEdit.value) showChooser()
  } else if (event.key.toLowerCase() === 'f' && library.value) {
    event.preventDefault()
    searchInput.value?.focus()
    searchInput.value?.select()
  }
}

onMounted(() => {
  window.addEventListener('keydown', onShortcut)
  if (isMac) {
    void listen('open-diagnostics', () => openPanel('diagnostics')).then(unlisten => {
      unlistenDiagnostics = unlisten
    }).catch(() => {
      // Web 构建预览不提供系统菜单事件。
    })
  }
  void run(async () => {
    const result = await invoke<LibraryTransition | null>('resume_library')
    if (result) applyTransition(result)
  }, '正在打开上次使用的目录…')
})

onBeforeUnmount(() => {
  window.removeEventListener('keydown', onShortcut)
  unlistenDiagnostics?.()
  window.clearTimeout(cleanupTimer)
  window.clearTimeout(noticeTimer)
})
</script>

<template>
  <div class="desktop" @click="closeMenu()">
    <div class="desktop-surface" :aria-hidden="panel ? true : undefined">
      <header class="toolbar">
        <div class="toolbar-title">
          <button class="icon-button" type="button" aria-label="显示或收起侧边栏" title="显示或收起侧边栏"
            @click="toggleSidebar"><DesktopIcon name="sidebar" /></button>
          <DesktopIcon name="folder" class="toolbar-folder" />
          <span class="truncate" :title="pathLabel">{{ libraryName }}</span>
        </div>
        <div class="toolbar-actions">
          <label class="search-field">
            <DesktopIcon name="search" />
            <input ref="searchInput" v-model="query" type="search" aria-label="筛选当前列表" placeholder="搜索" :disabled="!library" />
            <kbd>{{ shortcut }}F</kbd>
          </label>
          <button class="icon-button" type="button" aria-label="显示或收起详情" title="显示或收起详情"
            :disabled="!library" @click="toggleInspector"><DesktopIcon name="info" /></button>
          <div class="menu-anchor" @click.stop>
            <button class="icon-button" type="button" aria-label="更多操作" title="更多操作"
              :aria-expanded="activeMenu?.kind === 'toolbar'" :disabled="!canEdit" @click="showMenu({ kind: 'toolbar' }, $event)">
              <DesktopIcon name="more" />
            </button>
            <div v-if="activeMenu?.kind === 'toolbar'" class="action-menu toolbar-menu" @keydown="onMenuKey">
              <button type="button" @click="showChooser"><DesktopIcon name="open" />打开目录 <kbd>{{ shortcut }}O</kbd></button>
              <template v-if="library">
                <button type="button" @click="closeMenu(); openPath(activeModule === 'files' ? folder || undefined : undefined)">
                  <DesktopIcon name="external" />{{ revealLabel }}
                </button>
                <button type="button" @click="copyPath(activeModule === 'files' ? folder : undefined)"><DesktopIcon name="copy" />复制目录路径</button>
                <div class="menu-rule"></div>
                <button v-if="activeModule === 'books'" type="button" @click="showReading('scan')"><DesktopIcon name="scan" />扫描小说</button>
                <button type="button" @click="showReading('save')"><DesktopIcon name="check" />保存外部改动</button>
                <button type="button" @click="openPanel('storage')"><DesktopIcon name="storage" />空间管理</button>
              </template>
              <button type="button" @click="openPanel('diagnostics')"><DesktopIcon name="settings" />诊断</button>
            </div>
          </div>
        </div>
      </header>

      <p v-if="error && !panel" class="workspace-error" role="alert">
        <DesktopIcon name="warning" /><span>{{ error }}</span>
        <button type="button" class="icon-button" aria-label="关闭错误提示" @click="error = ''"><DesktopIcon name="close" /></button>
      </p>

      <div class="desktop-layout" :data-sidebar="sidebar" :data-inspector="inspector" :data-selection="!!selection">
        <button class="sidebar-scrim" type="button" aria-label="关闭侧边栏" @click="sidebar = 'hidden'"></button>
        <aside class="sidebar" aria-label="侧边栏">
          <nav class="side-group" aria-label="模块">
            <div class="side-label">模块</div>
            <button v-for="(name, module) in modules" :key="module" class="side-row" type="button"
              :aria-current="activeModule === module ? 'page' : undefined" :disabled="!canEdit" @click="selectModule(module)">
              <DesktopIcon :name="module === 'files' ? 'folder' : 'books'" />{{ name }}
            </button>
          </nav>
          <section v-if="library" class="side-group" aria-label="分支">
            <div class="side-label">分支
              <button class="icon-button" type="button" aria-label="新建分支" title="新建分支"
                :disabled="!canEdit" @click="showBranchEditor"><DesktopIcon name="plus" /></button>
            </div>
            <div v-for="name in library.branches" :key="name" class="branch-row" :data-current="name === currentBranch">
              <button class="side-row branch-button" type="button" :title="name"
                :aria-current="name === currentBranch ? 'true' : undefined"
                :disabled="!canEdit" @click="name !== currentBranch && switchBranch(name)">
                <DesktopIcon name="branch" /><span class="truncate">{{ name }}</span>
                <span v-if="name === currentBranch" class="branch-state">当前</span>
                <span v-else-if="name === library.defaultBranch" class="branch-state">默认</span>
                <span v-if="name === currentBranch && name === library.defaultBranch" class="sr-only">默认</span>
              </button>
              <div class="menu-anchor branch-anchor" @click.stop>
                <button class="icon-button" type="button" :aria-label="`${name} 的分支操作`"
                  title="分支操作" :aria-expanded="activeMenu?.kind === 'branch' && activeMenu.branch === name"
                  :disabled="!canEdit" @click="showMenu({ kind: 'branch', branch: name }, $event)"><DesktopIcon name="more" /></button>
                <div v-if="activeMenu?.kind === 'branch' && activeMenu.branch === name" class="action-menu branch-menu" @keydown="onMenuKey">
                  <button v-if="name !== library.defaultBranch" type="button" @click="setDefault(name)">
                    <DesktopIcon name="check" />设为默认分支
                  </button>
                  <span v-else class="menu-caption">已是默认分支</span>
                </div>
              </div>
            </div>
          </section>
        </aside>

        <main class="desktop-main">
          <template v-if="library">
            <header class="content-heading">
              <div class="heading-text">
                <div class="heading-title">
                  <button v-if="activeModule === 'files' && folder" class="icon-button back-button" type="button" aria-label="返回上级"
                    @click="navigateFolder(crumbs.at(-2)?.path ?? '')"><DesktopIcon name="back" /></button>
                  <h1 class="truncate" :title="folder">{{ activeModule === 'books' ? '全部书目' : fileName(folder) || '全部文件' }}</h1>
                </div>
                <p><DesktopIcon name="branch" /><span class="truncate">{{ currentBranch }}</span>
                  <template v-if="library.defaultBranch === currentBranch"><span>·</span><span>默认分支</span></template>
                </p>
              </div>
              <div class="heading-actions">
                <template v-if="activeModule === 'books'">
                  <button type="button" class="button scan-action" :disabled="!canEdit" @click="showReading('scan')">
                    <DesktopIcon name="scan" />扫描小说
                  </button>
                  <button type="button" class="button" :disabled="!canEdit" @click="showReading('import')">
                    <DesktopIcon name="import" />导入更新
                  </button>
                </template>
                <template v-else>
                  <button type="button" class="button" :disabled="!canEdit" @click="showReading('import')">
                    <DesktopIcon name="import" />读取来源目录
                  </button>
                  <button type="button" class="icon-button" :title="revealLabel" :aria-label="revealLabel"
                    :disabled="!canEdit" @click="openPath(folder || undefined)"><DesktopIcon name="open" /></button>
                </template>
              </div>
            </header>
            <div v-if="recoveryNotice" class="recovery-notice" role="status">
              <DesktopIcon name="check" /><span>{{ recoveryNotice }}</span>
              <button type="button" class="icon-button" aria-label="关闭恢复提示" @click="recoveryNotice = ''"><DesktopIcon name="close" /></button>
            </div>
            <div v-if="selection" class="selection-bar">
              <span>已选 {{ selectedCount }} 项</span>
              <div>
                <button v-if="selectedFolder !== undefined" class="button" type="button" :disabled="!canEdit"
                  @click="navigateFolder(selectedFolder)">打开文件夹</button>
                <button v-if="singleFile" class="button" type="button" :disabled="!canEdit" @click="showHistory(singleFile)">查看版本</button>
                <button v-else-if="pickedBook?.files[0]" class="button" type="button" :disabled="!canEdit" @click="showHistory(pickedBook.files[0], pickedBook)">查看版本</button>
                <button v-if="canMove" class="button" type="button" :disabled="!canEdit" @click="showMove">移至分支</button>
                <button v-if="canGroup" class="button" type="button" :disabled="!canEdit" @click="showCreateBook">创建书目</button>
              </div>
            </div>
            <LibraryBrowser ref="browser" :library="library" :module="activeModule" :folder="folder"
              :query="query" :selection="selection" @select="selection = $event" @enter-folder="navigateFolder"
              @open-file="openPath" @read="showReading(activeModule === 'books' ? 'scan' : 'import')" />
          </template>
          <div v-else class="empty-state start-state">
            <DesktopIcon :name="activeModule === 'books' ? 'books' : 'open'" />
            <h1>{{ busy ? '正在打开管理目录' : activeModule === 'books' ? '打开书库目录' : '打开管理目录' }}</h1>
            <p v-if="!busy">选择一个本地文件夹开始整理。</p>
            <button ref="openButton" type="button" class="button primary" :disabled="!canEdit" @click="showChooser">选择目录</button>
          </div>
        </main>

        <aside v-if="library" class="inspector" aria-label="详情">
          <header><span>详情</span>
            <button class="icon-button" type="button" aria-label="关闭详情" @click="inspector = 'hidden'"><DesktopIcon name="close" /></button>
          </header>
          <div class="inspector-hero">
            <span v-if="pickedBook" class="book-glyph"><DesktopIcon name="book" /></span>
            <span v-else-if="singleFile" class="file-glyph">{{ fileSuffix(singleFile).toUpperCase().slice(0, 8) }}</span>
            <DesktopIcon v-else :name="selectedFiles.length > 1 ? 'file' : 'folder'" />
            <h2>{{ pickedBook?.title ?? (pickedPath ? fileName(pickedPath) : selectedFiles.length > 1 ? `${selectedFiles.length} 个文件` : fileName(folder) || libraryName) }}</h2>
            <p>{{ pickedBook?.author || (pickedBook ? `${pickedBook.files.length} 个文件` : singleFile ? (fileSuffix(singleFile).toUpperCase() || '文件') : selectedFiles.length > 1 ? '' : '文件夹') }}</p>
          </div>
          <dl class="inspector-meta">
            <dt>分支</dt><dd>{{ currentBranch }}</dd>
            <template v-if="singleFile && library.novels[singleFile]?.progress">
              <dt>进度</dt><dd>{{ library.novels[singleFile]?.progress }}</dd>
            </template>
            <template v-if="pickedPath || !selection">
              <dt>路径</dt><dd :title="absolutePath(pickedPath ?? folder)">{{ absolutePath(pickedPath ?? folder) }}</dd>
            </template>
          </dl>
          <div v-if="pickedBook" class="inspector-files">
            <button v-for="path in pickedBook.files" :key="path" class="inspector-file" type="button"
              :title="path" :disabled="!canEdit" @click="showHistory(path, pickedBook)">
              <span class="truncate">{{ fileName(path) }}</span><span class="format-tag">{{ fileSuffix(path).toUpperCase() || '文件' }}</span>
              <DesktopIcon name="right" />
            </button>
          </div>
          <div class="inspector-actions">
            <button v-if="singleFile" class="button" type="button" :disabled="!canEdit" @click="openPath(singleFile)"><DesktopIcon name="external" />打开文件</button>
            <button v-if="selectedFolder !== undefined" class="button" type="button" :disabled="!canEdit"
              @click="navigateFolder(selectedFolder)"><DesktopIcon name="folder" />打开文件夹</button>
            <button v-if="singleFile || pickedBook?.files[0]" class="button" type="button" :disabled="!canEdit"
              @click="showHistory(singleFile ?? pickedBook!.files[0])"><DesktopIcon name="history" />查看版本</button>
            <button v-if="canMove" class="button" type="button" :disabled="!canEdit" @click="showMove"><DesktopIcon name="move" />移至分支</button>
            <button v-if="canGroup" class="button" type="button" :disabled="!canEdit" @click="showCreateBook"><DesktopIcon name="plus" />创建书目</button>
            <button v-if="pickedPath || !selection" class="button" type="button" :disabled="!canEdit"
              @click="openPath(pickedPath ?? (folder || undefined), !!pickedPath)"><DesktopIcon name="open" />{{ revealLabel }}</button>
            <button v-if="pickedPath || !selection" class="button" type="button" :disabled="!canEdit"
              @click="copyPath(pickedPath ?? folder)"><DesktopIcon name="copy" />复制路径</button>
          </div>
        </aside>
      </div>

      <footer class="status-bar">
        <nav v-if="library" class="breadcrumbs" aria-label="目录路径" :title="absolutePath(activeModule === 'files' ? folder : '')">
          <DesktopIcon name="folder" />
          <button type="button" class="truncate" @click="navigateFolder('')">{{ libraryName }}</button>
          <template v-if="activeModule === 'files'">
            <template v-for="crumb in crumbs" :key="crumb.path">
              <DesktopIcon name="right" /><button type="button" class="truncate" @click="navigateFolder(crumb.path)">{{ crumb.name }}</button>
            </template>
          </template>
        </nav>
        <span v-if="task || collecting" role="status" class="task-state">
          <span class="spinner" aria-hidden="true"></span>{{ task || '正在回收空间…' }}
        </span>
        <span v-else-if="library" class="status-count">
          {{ activeModule === 'files' ? `${visibleFileCount} 个文件` : `${bookCount} 个书目` }}
          <template v-if="selectedCount"> · 已选 {{ selectedCount }} 项</template>
          · {{ currentBranch }}
        </span>
      </footer>
    </div>

    <div v-if="notice && !panel" class="toast" role="status"><DesktopIcon name="check" />{{ notice }}</div>

    <DesktopSheet v-if="panel" :title="title" :busy="busy" :wide="panel === 'reading' || panel === 'diagnostics'"
      @close="closePanel" @return-focus="returnFocus">
      <LibraryReading v-if="panel === 'reading' && library" :library="library" :busy="busy"
        :run="runReading" :module="activeModule" :entry="readingEntry"
        @updated="updateLibrary" @close="closePanel" @complete="completeReading">
        <p v-if="error" class="inline-error" role="alert">{{ error }}</p>
        <p v-if="busy" class="sheet-task" role="status">{{ task }}</p>
      </LibraryReading>

      <template v-else>
        <div class="sheet-body">
          <template v-if="panel === 'chooser'">
            <p>选中的目录由两个模块共用。</p>
            <div class="chooser-path">
              <button class="button" type="button" data-initial-focus :disabled="busy" @click="chooseDirectory"><DesktopIcon name="open" />选择目录</button>
              <span class="break-text">{{ chosenDirectory || '尚未选择目录' }}</span>
            </div>
            <label class="check-field"><input v-model="createMode" type="checkbox" :disabled="busy" />在所选目录下新建独立管理目录</label>
            <label v-if="createMode" class="field">
              <span>目录名称</span>
              <input v-model="directoryName" type="text" :disabled="busy" @keydown.enter="openLibrary" />
            </label>
          </template>

          <label v-else-if="panel === 'branch'" class="field">
            <span>分支名称</span>
            <input v-model="branchName" type="text" data-initial-focus :disabled="busy" @keydown.enter="createBranch" />
          </label>

          <label v-else-if="panel === 'book'" class="field">
            <span>书目名称</span>
            <input v-model="bookTitle" type="text" data-initial-focus :disabled="busy" @keydown.enter="createBook" />
          </label>

          <template v-else-if="panel === 'switch' && pending">
            <p>将以下改动保存到「{{ pending.sourceBranch }}」，然后切换到「{{ pending.targetBranch }}」。</p>
            <ul class="sheet-list change-list">
              <li v-for="change in pending.changes" :key="change.path">
                <span class="format-tag">{{ changeLabels[change.kind] }}</span>
                <span>{{ change.previousPath ? `${change.previousPath} → ` : '' }}{{ change.path }}</span>
              </li>
            </ul>
          </template>

          <template v-else-if="panel === 'move'">
            <div class="move-route">
              <div class="field"><span>源分支</span><strong>{{ currentBranch }}</strong></div>
              <DesktopIcon name="move" />
              <label class="field">
                <span>目标分支</span>
                <select v-model="moveTarget" data-initial-focus :disabled="busy" @change="changeMoveTarget">
                  <option value="">选择目标分支</option>
                  <option v-for="name in otherBranches" :key="name">{{ name }}</option>
                </select>
              </label>
            </div>
            <template v-if="movePending?.status.kind === 'saveExternal'">
              <p>请先保存「{{ currentBranch }}」的改动，保存后会重新预览移动内容。</p>
              <ul class="sheet-list change-list">
                <li v-for="change in movePending.status.changes" :key="change.path">
                  <span class="format-tag">{{ changeLabels[change.kind] }}</span>
                  <span>{{ change.previousPath ? `${change.previousPath} → ` : '' }}{{ change.path }}</span>
                </li>
              </ul>
            </template>
            <template v-else-if="movePending?.status.kind === 'preview'">
              <p>移动 {{ movePending.status.items.length }} 项。确认后两个分支会一起更新。</p>
              <ul class="sheet-list move-items">
                <li v-for="item in movePending.status.items" :key="item.path">
                  <div class="move-file"><DesktopIcon name="file" /><span class="break-text">{{ item.path }}</span></div>
                  <div v-if="item.conflict" class="conflict">
                    <p><DesktopIcon name="warning" />与目标分支的「{{ item.conflict }}」冲突。</p>
                    <label class="field">
                      <span>新的目标相对路径</span>
                      <input v-model="newDestinations[item.path]" type="text" :disabled="busy"
                        :aria-label="`${item.path} 的新目标路径`" :aria-invalid="!!newDestinations[item.path] && !!destinationError(item.path)" />
                    </label>
                    <div class="conflict-actions">
                      <button type="button" class="text-button" :disabled="busy" @click="suggestDestination(item.path)">使用建议名称</button>
                      <small v-if="newDestinations[item.path] && destinationError(item.path)">{{ destinationError(item.path) }}</small>
                    </div>
                  </div>
                </li>
              </ul>
            </template>
            <template v-else>
              <p>{{ pickedBook ? `《${pickedBook.title}》 · 整本` : `已选 ${selectedCount} 个文件` }}</p>
              <ul class="sheet-list move-items">
                <li v-for="path in pickedBook?.files ?? selectedFiles" :key="path"><div class="move-file">
                  <DesktopIcon name="file" /><span class="break-text">{{ path }}</span>
                </div></li>
              </ul>
            </template>
          </template>

          <template v-else-if="panel === 'history' && history">
            <template v-if="restoreChoice">
              <p>将所选旧版恢复至「{{ currentBranch }}」，并保存为新的当前版。</p>
              <dl class="restore-preview">
                <dt>所选旧版</dt><dd>{{ restoreChoice.savedPath }}</dd>
                <dt>恢复位置</dt><dd>{{ restoreChoice.destination }}</dd>
              </dl>
            </template>
            <template v-else>
              <p class="history-name break-text">{{ history.book?.title ?? fileName(history.path) }}</p>
              <div v-if="historyTabs.length" class="format-tabs" role="group" aria-label="历史格式">
                <button v-for="tab in historyTabs" :key="tab.path" type="button"
                  :aria-pressed="tab.path === history.path" :disabled="busy"
                  @click="tab.path !== history.path && showHistory(tab.path, history.book)">{{ tab.label }}</button>
              </div>
              <section class="history-group" aria-labelledby="current-versions">
                <h3 id="current-versions">各分支当前版</h3>
                <ul class="sheet-list history-list">
                  <li v-for="file in history.data.current" :key="file.branch + '/' + file.path">
                    <span class="format-tag">{{ file.branch }}</span><span>{{ file.path }}</span>
                  </li>
                </ul>
              </section>
              <section class="history-group" aria-labelledby="old-versions">
                <h3 id="old-versions">可恢复旧版 <span>{{ history.data.versions.length }} / 10</span></h3>
                <ul v-if="history.data.versions.length" class="sheet-list history-list">
                  <li v-for="version in history.data.versions" :key="version.order">
                    <span>{{ version.savedPath }}</span>
                    <button type="button" class="button" :disabled="busy" @click="restoreChoice = version">选择恢复</button>
                  </li>
                </ul>
                <p v-else>此版本范围暂无旧版。</p>
              </section>
            </template>
          </template>

          <template v-else-if="panel === 'storage'">
            <p>各分支仍使用的版本会继续保留。回收会释放已超出保留上限的旧版对象占用。</p>
            <p v-if="cleanupNotice" class="storage-result" role="status">{{ cleanupNotice }}</p>
          </template>

          <EngineVerification v-else-if="panel === 'diagnostics'" />

          <p v-if="error" class="inline-error" role="alert">{{ error }}</p>
          <p v-if="busy" class="sheet-task" role="status">{{ task }}</p>
        </div>
        <footer class="sheet-footer">
          <button v-if="panel === 'history' && restoreChoice" class="button" type="button" :disabled="busy"
            @click="restoreChoice = null; error = ''">返回版本列表</button>
          <button v-else class="button" type="button" :disabled="busy" @click="closePanel">
            {{ panel === 'history' || panel === 'storage' || panel === 'diagnostics' ? '关闭' : '取消' }}
          </button>
          <button v-if="panel === 'chooser'" class="button primary" type="button"
            :disabled="busy || !chosenDirectory || (createMode && !directoryName.trim())" @click="openLibrary">
            {{ createMode ? '创建独立目录' : '打开或直接纳管' }}
          </button>
          <button v-else-if="panel === 'branch'" class="button primary" type="button" :disabled="busy || !branchName.trim()" @click="createBranch">创建空分支</button>
          <button v-else-if="panel === 'book'" class="button primary" type="button" :disabled="busy || !bookTitle.trim()" @click="createBook">创建书目</button>
          <button v-else-if="panel === 'switch' && pending" class="button primary" type="button" :disabled="busy"
            @click="switchBranch(pending.targetBranch, pending.token)">保存并切换</button>
          <button v-else-if="panel === 'move' && !movePending" class="button primary" type="button" :disabled="busy || !moveTarget"
            @click="startMove">预览移动</button>
          <button v-else-if="panel === 'move' && movePending" class="button primary" type="button" :disabled="busy || !canConfirmMove"
            @click="confirmMove">{{ movePending.status.kind === 'saveExternal' ? '保存外部改动' : '确认移动' }}</button>
          <button v-else-if="panel === 'history' && restoreChoice" class="button primary" type="button" :disabled="busy"
            @click="restoreSelectedVersion">恢复所选旧版</button>
          <button v-else-if="panel === 'storage'" class="button primary" type="button" :disabled="busy"
            @click="cleanupNow">{{ collecting ? '正在回收…' : '回收空间' }}</button>
        </footer>
      </template>
    </DesktopSheet>
  </div>
</template>

