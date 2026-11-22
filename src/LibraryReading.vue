<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { computed, onMounted, reactive, ref, shallowRef, watch } from 'vue'
import DesktopIcon from './DesktopIcon.vue'
import { changeLabels, defaultFormats, fileName, optionalFormats, type ExternalChange, type Library, type NovelFormat, type NovelName, type WorkspaceModule } from './library-types'

const props = defineProps<{
  library: Library
  busy: boolean
  run: (operation: () => Promise<void>) => Promise<void>
  module: WorkspaceModule
  entry: 'import' | 'save'
}>()
const emit = defineEmits<{
  close: []
  updated: [library: Library]
  complete: [message: string]
}>()

type Source = { kind: 'library' } | { kind: 'directory'; directory: string }
type Relation = 'same' | 'newer' | 'older' | 'review'

interface ReadIntent {
  source: Source
  formats: NovelFormat[] | null
}

interface ReadLocation {
  branch: string
  path: string
  relation: Relation
}

interface ReadBook {
  key: string
  title: string
  author: string | null
  existing: boolean
  locations: ReadLocation[]
}

interface ReadItem {
  path: string
  novel: NovelName | null
  books: ReadBook[]
  preferredBook: string | null
  supersededBy: string | null
  coveringPaths: string[]
  samePaths: ReadLocation[]
}

interface ReadSelection {
  path: string
  book: string | null
  branch: string
  action: 'keep' | 'history'
  destination: string
  replaces: string[]
}

type ActionSetting = 'recommended' | ReadSelection['action']

interface ReadChoice extends Omit<ReadSelection, 'action' | 'replaces'> {
  branchChosen: boolean
  action: ActionSetting
  replaces: string[] | null
}

interface ReadPreview {
  source: string
  token: string
  externalChanges: ExternalChange[]
  items: ReadItem[]
}

type Confirmation =
  | { kind: 'saveExternal'; token: string }
  | { kind: 'apply'; token: string; selections: ReadSelection[] }

interface ReadTransition {
  library: Library
  status: { kind: 'preview'; preview: ReadPreview } | { kind: 'savedExternal' } | { kind: 'applied' }
}

const relationLabels: Record<Relation, string> = {
  same: '内容相同',
  newer: '正文覆盖更完整',
  older: '现有正文覆盖此文件',
  review: '需要比较',
}
const novelMode = computed(() => props.module === 'books')
const allFormats = [...defaultFormats, ...optionalFormats]
const formats = ref<NovelFormat[]>([...defaultFormats])
// 当前预览和来源参数成对保留，确认时使用同一次扫描的票据。
const reading = shallowRef<{ intent: ReadIntent; preview: ReadPreview } | null>(null)
const selected = reactive(new Map<string, ReadChoice>())
const activePath = ref('')
const search = ref('')
const message = ref('')
const batchBranch = ref('')
const batchAction = ref<ActionSetting | ''>('')
const local = computed(() => reading.value?.intent.source.kind === 'library')
const filtered = computed(() => {
  const query = search.value.trim().toLowerCase()
  return (reading.value?.preview.items ?? []).filter(item =>
    !query || [item.path, item.novel?.title, item.novel?.author]
      .some(value => value?.toLowerCase().includes(query)),
  )
})
const itemsByPath = computed(() => new Map(reading.value?.preview.items.map(item => [item.path, item])))
// 将界面设置转成同一次读取请求的处理清单，按现有 Rust 票据一次提交。
const plans = computed(() => {
  const actions = new Map<string, ReadSelection['action']>()
  for (const choice of selected.values()) {
    const item = itemsByPath.value.get(choice.path)
    if (item) actions.set(choice.path, choice.action === 'recommended' ? recommendedAction(item, choice) : choice.action)
  }
  return new Map([...selected.values()].flatMap(choice => {
    const item = itemsByPath.value.get(choice.path)
    const action = actions.get(choice.path)
    if (!item || !action) return []
    const replaces = action === 'keep'
      ? (choice.replaces ?? suggestedReplacements(item, choice)).filter(path => !local.value || actions.get(path) !== 'keep')
      : []
    const plan: ReadSelection = {
      path: choice.path, book: choice.book, branch: choice.branch, action, destination: choice.destination, replaces,
    }
    return [[choice.path, plan] as const]
  }))
})
const visible = computed(() => filtered.value.map(item => ({ item, choice: selected.get(item.path), plan: plans.value.get(item.path) })))
const active = computed(() => visible.value.find(row => row.item.path === activePath.value) ?? visible.value[0])
const activeReplacements = computed(() => {
  const row = active.value
  return row?.choice && row.plan?.action === 'keep'
    ? replacementOptions(row.item, row.choice).filter(location => !local.value || plans.value.get(location.path)?.action !== 'keep')
    : []
})
const allReplaced = computed(() => activeReplacements.value.length > 0
  && activeReplacements.value.every(location => active.value?.plan?.replaces.includes(location.path)))
const allSelected = computed(() => filtered.value.length > 0 && filtered.value.every(item => selected.has(item.path)))
function formatGroup(plan: ReadSelection) {
  return JSON.stringify([plan.branch, plan.book, itemsByPath.value.get(plan.path)?.novel?.format])
}

const review = computed(() => {
  const kept = new Set<string>()
  const removed = new Map<string, Set<string>>()
  for (const plan of plans.value.values()) {
    if (plan.book && plan.action === 'keep') kept.add(formatGroup(plan))
    const paths = removed.get(plan.branch) ?? new Set<string>()
    for (const path of plan.replaces) paths.add(path)
    if (plan.action === 'history' && local.value) paths.add(plan.path)
    removed.set(plan.branch, paths)
  }
  const pending = new Map<string, string>()
  for (const plan of plans.value.values()) {
    if (novelMode.value && !plan.book) pending.set(plan.path, '请确认所属书目。')
    else if (!plan.branch) pending.set(plan.path, '请选择目标分支。')
    else if (plan.action === 'keep' && !plan.destination.trim()) pending.set(plan.path, '请填写目标相对路径。')
    else if (plan.action === 'history' && !kept.has(formatGroup(plan))) {
      const item = itemsByPath.value.get(plan.path)
      const choice = selected.get(plan.path)
      if (item && choice && !forBranch(item, choice).some(location => !removed.get(plan.branch)?.has(location.path))) {
        pending.set(plan.path, '请在目标分支保留一个同书同格式的当前文件。')
      }
    }
  }
  return pending
})
const unresolved = computed(() => review.value.size)
const canScan = computed(() => !props.busy && (!novelMode.value || formats.value.length > 0))
const canApply = computed(() => !props.busy && plans.value.size > 0 && unresolved.value === 0)

watch(filtered, items => {
  const paths = new Set(items.map(item => item.path))
  for (const path of selected.keys()) {
    if (!paths.has(path)) selected.delete(path)
  }
})

function clearBatchSettings() {
  batchBranch.value = ''
  batchAction.value = ''
}

function resetPreview() {
  reading.value = null
  selected.clear()
  activePath.value = ''
  search.value = ''
  message.value = ''
  clearBatchSettings()
}

function toggleFormats() {
  formats.value = formats.value.length === allFormats.length ? [] : [...allFormats]
  resetPreview()
}

function intentFor(source: Source): ReadIntent {
  return { source, formats: novelMode.value ? [...formats.value] : null }
}

async function request(intent: ReadIntent, confirmation?: Confirmation) {
  const directory = props.library.directory
  const result = await invoke<ReadTransition>('read_directory', {
    directory,
    request: { ...intent, confirmation },
  })
  emit('updated', result.library)
  if (result.status.kind === 'savedExternal') {
    if (props.entry === 'save') emit('complete', `已保存「${result.library.currentBranch}」的外部改动。`)
    else await request(intent)
    return
  }
  if (result.status.kind === 'preview') {
    selected.clear()
    activePath.value = ''
    clearBatchSettings()
    reading.value = { intent, preview: result.status.preview }
    message.value = confirmation ? '来源或当前分支已变化，请重新核对预览。' : ''
  } else {
    const branches = [...new Set([...selected.values()].map(item => `「${item.branch}」`))].join('、')
    emit('complete', `已将 ${selected.size} 项保存至${branches}。`)
  }
}

onMounted(() => {
  if (props.entry === 'save') void props.run(() => request({ source: { kind: 'library' }, formats: null }))
})

function chooseSource() {
  return props.run(async () => {
    const directory = await invoke<string | null>('choose_library_directory', { title: '选择读取来源' })
    if (directory) await request(intentFor({ kind: 'directory', directory }))
  })
}

function locations(item: ReadItem, choice: ReadChoice): ReadLocation[] {
  return item.novel ? item.books.find(book => book.key === choice.book)?.locations ?? [] : item.samePaths
}

function forBranch(item: ReadItem, choice: ReadChoice): ReadLocation[] {
  return locations(item, choice).filter(location => location.branch === choice.branch)
}

function recommendedAction(item: ReadItem, choice: ReadChoice): ReadSelection['action'] {
  if (!item.novel || !choice.book || !choice.branch) return 'keep'
  if (!local.value && forBranch(item, choice).some(location => location.relation === 'same' || location.relation === 'older')) return 'history'
  if (choice.book !== item.preferredBook) return 'keep'
  // Rust 已判定同书同格式的覆盖关系。跨目录读取只使用本批选到同一分支的候选。
  return item.coveringPaths.some(path => {
    const newer = selected.get(path)
    if (!newer) return local.value
    return newer.branch === choice.branch && newer.book === choice.book && newer.action !== 'history'
  }) ? 'history' : 'keep'
}

function route(item: ReadItem, choice: ReadChoice) {
  if (!choice.branchChosen) {
    const candidates = [...new Set(locations(item, choice).map(location => location.branch))]
    choice.branch = local.value || !item.novel || candidates.length === 0
      ? props.library.currentBranch
      : candidates.length === 1 ? candidates[0] : ''
  }
  choice.replaces = null
}

function replacementOptions(item: ReadItem, choice: ReadChoice) {
  return forBranch(item, choice).filter(location => item.novel || location.path === choice.destination)
}

function suggestedReplacements(item: ReadItem, choice: ReadChoice): string[] {
  return replacementOptions(item, choice).filter(location => item.novel
    ? location.relation === 'newer' || (local.value && location.relation === 'same')
    : location.relation !== 'same',
  ).map(location => location.path)
}

function ordinaryStatus(item: ReadItem, choice?: ReadChoice) {
  const current = item.samePaths.find(location => location.branch === (choice?.branch || props.library.currentBranch))
  if (!current) return '新增'
  return current.relation === 'same' ? '无变化' : '更新'
}

function selectItem(item: ReadItem) {
  if (selected.has(item.path)) return
  const choice: ReadChoice = {
    path: item.path, book: item.preferredBook, branch: '', branchChosen: false, action: 'recommended',
    destination: item.path, replaces: null,
  }
  route(item, choice)
  selected.set(item.path, choice)
}

function toggle(item: ReadItem) {
  activePath.value = item.path
  if (!selected.delete(item.path)) selectItem(item)
}

function selectAll() {
  if (props.busy) return
  for (const item of filtered.value) selectItem(item)
}

function toggleAll() {
  if (allSelected.value) selected.clear()
  else selectAll()
}

function onResultKey(event: KeyboardEvent) {
  if (!event.altKey && (event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'a') {
    event.preventDefault()
    selectAll()
  }
}

function applyBatchSettings() {
  if (props.busy || selected.size < 2) return
  for (const choice of selected.values()) {
    if (batchBranch.value && !local.value) {
      if (choice.branch !== batchBranch.value) choice.replaces = null
      choice.branch = batchBranch.value
      choice.branchChosen = true
    }
    if (batchAction.value && novelMode.value) {
      choice.action = batchAction.value
      choice.replaces = null
    }
  }
  clearBatchSettings()
}

function toggleReplacement(choice: ReadChoice, plan: ReadSelection, path: string) {
  choice.replaces = plan.replaces.includes(path)
    ? plan.replaces.filter(replaced => replaced !== path)
    : [...plan.replaces, path]
}

function toggleAllReplacements(choice: ReadChoice) {
  choice.replaces = allReplaced.value ? [] : activeReplacements.value.map(location => location.path)
}

function confirm(saveExternal: boolean) {
  const active = reading.value
  if (!active || (!saveExternal && !canApply.value)) return
  const confirmation: Confirmation = saveExternal
    ? { kind: 'saveExternal', token: active.preview.token }
    : { kind: 'apply', token: active.preview.token, selections: [...plans.value.values()] }
  return props.run(() => request(active.intent, confirmation))
}
</script>

<template>
  <div class="sheet-body reading-body">
    <template v-if="entry !== 'save'">
      <p>{{ novelMode ? '选择下载目录，检查书目、格式及目标分支。' : '选择来源目录，按相对路径查看新增和更新。' }}</p>
      <fieldset v-if="novelMode" class="read-formats" :disabled="busy">
        <legend>文件格式</legend>
        <button class="text-button" type="button" @click="toggleFormats">{{ formats.length === allFormats.length ? '取消全选格式' : '全选格式' }}</button>
        <label v-for="format in allFormats" :key="format">
          <input v-model="formats" type="checkbox" :value="format" @change="resetPreview" />
          <span>.{{ format }}</span>
        </label>
      </fieldset>
      <button class="button" type="button" data-initial-focus :disabled="!canScan" @click="chooseSource">
        <DesktopIcon name="open" />{{ reading ? '更换来源目录' : '选择来源目录' }}
      </button>
    </template>
    <p v-if="message" class="inline-warning" role="status">{{ message }}</p>
    <template v-if="reading">
      <div v-if="entry !== 'save'" class="read-summary">
        <span class="truncate" :title="reading.preview.source">{{ reading.preview.source }}</span>
        <span>{{ reading.preview.items.length }} 项 · 已选 {{ selected.size }} 项</span>
      </div>
      <template v-if="reading.preview.externalChanges.length">
        <p>先将以下改动保存至「{{ library.currentBranch }}」。</p>
        <ul class="sheet-list change-list">
          <li v-for="change in reading.preview.externalChanges" :key="change.path">
            <span class="format-tag">{{ changeLabels[change.kind] }}</span>
            <span>{{ change.previousPath ? change.previousPath + ' → ' : '' }}{{ change.path }}</span>
          </li>
        </ul>
      </template>
      <p v-else-if="entry === 'save'">「{{ library.currentBranch }}」没有待保存的外部改动。</p>
      <template v-else-if="reading.preview.items.length">
        <div class="read-tools">
          <label class="search-field read-search">
            <DesktopIcon name="search" />
            <input v-model="search" type="search" aria-label="筛选扫描结果" placeholder="筛选扫描结果" :disabled="busy" />
          </label>
          <button class="button" type="button" :disabled="busy || !filtered.length" @click="toggleAll">
            {{ allSelected ? '取消全选' : '全选' }}
          </button>
          <span v-if="unresolved" class="read-needs-review" role="status">{{ unresolved }} 项待确认</span>
        </div>
        <fieldset v-if="selected.size > 1" class="read-batch" :disabled="busy">
          <legend>批量设置</legend>
          <label v-if="!local" class="field">
            <span>批量目标分支</span>
            <select v-model="batchBranch">
              <option value="">保留各自分支设置</option>
              <option v-for="branch in library.branches" :key="branch">{{ branch }}</option>
            </select>
          </label>
          <label v-if="novelMode" class="field">
            <span>批量处理方式</span>
            <select v-model="batchAction">
              <option value="">保留各自处理方式</option>
              <option value="recommended">按推荐处理</option>
              <option value="keep">保留为当前文件</option>
              <option value="history">保存为旧版</option>
            </select>
          </label>
          <button class="button" type="button" :disabled="!batchBranch && !batchAction" @click="applyBatchSettings">应用至已选</button>
        </fieldset>
        <div v-if="active" class="read-review">
          <ul class="sheet-list read-results" aria-label="扫描候选" @keydown="onResultKey">
            <li v-for="{ item, choice, plan } in visible" :key="item.path" :data-active="item.path === active.item.path">
              <input type="checkbox" :checked="!!choice" :disabled="busy" :aria-label="'选择 ' + item.path" @change="toggle(item)" />
              <button class="read-item" type="button" :aria-pressed="item.path === active.item.path" @click="activePath = item.path">
                <span>
                  <strong>{{ item.novel?.title || fileName(item.path) }}</strong>
                  <small class="break-text">{{ item.path }}</small>
                  <small v-if="plan && !review.has(item.path)">{{ plan.branch }} · {{ plan.action === 'history' ? '保存为旧版' : '保留为当前文件' }}</small>
                </span>
                <span class="format-tag">{{ item.novel?.format.toUpperCase() ?? ordinaryStatus(item, choice) }}</span>
              </button>
              <span v-if="review.has(item.path)" class="read-needs-review" :title="review.get(item.path)">待确认</span>
            </li>
          </ul>

          <section class="read-detail" aria-label="候选详情">
            <h3 class="break-text">{{ active.item.novel?.title || fileName(active.item.path) }}</h3>
            <div v-if="active.item.novel" class="read-facts">
              <span>{{ active.item.novel.author || '作者未识别' }}</span>
              <span v-if="active.item.novel.progress">{{ active.item.novel.progress }}</span>
              <span v-if="active.item.novel.completed">完结</span>
              <span v-if="active.item.novel.revised">修订或特殊版本</span>
              <span v-if="active.item.novel.supplemental">补充内容</span>
              <span v-if="active.item.novel.copy !== null">下载副本 {{ active.item.novel.copy }}</span>
            </div>
            <p v-if="active.item.supersededBy" class="read-hint">建议保留「{{ active.item.supersededBy }}」为当前版。</p>
            <p v-if="review.has(active.item.path)" class="read-hint">{{ review.get(active.item.path) }}</p>
            <div v-if="active.choice && active.plan" class="read-choice">
              <label v-if="active.item.novel" class="field">
                <span>归入书目</span>
                <select v-model="active.choice.book" :disabled="busy" @change="route(active.item, active.choice)">
                  <option :value="null">确认所属书目</option>
                  <option v-for="book in active.item.books" :key="book.key" :value="book.key">
                    {{ book.title }} · {{ book.author || '作者未识别' }} · {{ book.existing ? '书库候选' : '新书目' }}{{ book.locations.length ? ' · ' + book.locations[0].path : '' }}
                  </option>
                </select>
              </label>
              <label class="field">
                <span>目标分支</span>
                <select v-model="active.choice.branch" :disabled="busy || local"
                  @change="active.choice.branchChosen = !!active.choice.branch; active.choice.replaces = null">
                  <option value="">指定本次更新的分支</option>
                  <option v-for="branch in library.branches" :key="branch">{{ branch }}</option>
                </select>
              </label>
              <label v-if="active.item.novel" class="field">
                <span>处理方式</span>
                <select v-model="active.choice.action" :disabled="busy" @change="active.choice.replaces = null">
                  <option value="recommended">按推荐 · {{ active.plan.action === 'history' ? '保存为旧版' : '保留为当前文件' }}</option>
                  <option value="keep">保留为当前文件</option>
                  <option value="history">保存为旧版</option>
                </select>
              </label>
              <label v-if="!local && active.plan.action === 'keep'" class="field">
                <span>目标相对路径</span>
                <input v-model="active.choice.destination" type="text" :disabled="busy" @input="active.choice.replaces = null" />
              </label>
              <fieldset v-if="activeReplacements.length" class="read-replacements" :disabled="busy">
                <legend>替换目标文件</legend>
                <button v-if="activeReplacements.length > 1" class="text-button" type="button" @click="toggleAllReplacements(active.choice)">
                  {{ allReplaced ? '取消全选替换项' : '全选替换项' }}
                </button>
                <label v-for="location in activeReplacements" :key="location.path">
                  <input type="checkbox" :checked="active.plan.replaces.includes(location.path)"
                    @change="toggleReplacement(active.choice, active.plan, location.path)" />
                  <span class="break-text">{{ location.path }}</span>
                  <small>{{ relationLabels[location.relation] }}</small>
                </label>
              </fieldset>
            </div>
            <p v-else class="read-unselected">勾选此项后可确认处理方式。</p>
          </section>
        </div>
        <p v-if="!visible.length">没有匹配的项目。</p>
      </template>
      <p v-else>此目录没有匹配的文件。</p>
    </template>
    <slot />
  </div>
  <footer class="sheet-footer">
    <button class="button" type="button" :disabled="busy" @click="$emit('close')">
      {{ entry === 'save' && reading && !reading.preview.externalChanges.length ? '关闭' : '取消' }}
    </button>
    <button v-if="reading?.preview.externalChanges.length" class="button primary" type="button" :disabled="busy" @click="confirm(true)">
      保存外部改动
    </button>
    <button v-else-if="entry !== 'save' && reading?.preview.items.length" class="button primary" type="button" :disabled="!canApply" @click="confirm(false)">
      {{ novelMode && entry === 'import' ? '导入所选' : '应用所选' }}
    </button>
  </footer>
</template>
