<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { computed, onMounted, reactive, ref, shallowRef } from 'vue'
import DesktopIcon from './DesktopIcon.vue'
import { changeLabels, defaultFormats, fileName, optionalFormats, type ExternalChange, type Library, type NovelFormat, type NovelName, type WorkspaceModule } from './library-types'

const props = defineProps<{
  library: Library
  busy: boolean
  run: (operation: () => Promise<void>) => Promise<void>
  module: WorkspaceModule
  entry: 'scan' | 'import' | 'save'
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
const formats = ref<NovelFormat[]>([...defaultFormats])
// 当前预览和来源参数成对保留，确认时使用同一次扫描的票据。
const reading = shallowRef<{ intent: ReadIntent; preview: ReadPreview } | null>(null)
const selected = reactive(new Map<string, ReadSelection>())
const search = ref('')
const message = ref('')
const local = computed(() => reading.value?.intent.source.kind === 'library')
const visible = computed(() => {
  const query = search.value.trim().toLowerCase()
  return (reading.value?.preview.items ?? []).filter(item =>
    !query || [item.path, item.novel?.title, item.novel?.author]
      .some(value => value?.toLowerCase().includes(query)),
  ).map(item => ({ item, choice: selected.get(item.path) }))
})
const canScan = computed(() => !props.busy && (!novelMode.value || formats.value.length > 0))
const canApply = computed(() => !props.busy && selected.size > 0
  && Array.from(selected.values()).every(choice =>
    !!choice.branch && (!novelMode.value || !!choice.book)
      && (choice.action !== 'keep' || !!choice.destination.trim()),
  ))

function resetPreview() {
  reading.value = null
  selected.clear()
  search.value = ''
  message.value = ''
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
    reading.value = { intent, preview: result.status.preview }
    message.value = confirmation ? '来源或当前分支已变化，请重新核对预览。' : ''
  } else {
    const branches = [...new Set([...selected.values()].map(item => `「${item.branch}」`))].join('、')
    emit('complete', `已将 ${selected.size} 项保存至${branches}。`)
  }
}

function scanCurrent() {
  return props.run(() => request(intentFor({ kind: 'library' })))
}

onMounted(() => {
  if (props.entry === 'save') void scanCurrent()
})

function chooseSource() {
  return props.run(async () => {
    const directory = await invoke<string | null>('choose_library_directory', { title: '选择读取来源' })
    if (directory) await request(intentFor({ kind: 'directory', directory }))
  })
}

function locations(item: ReadItem, choice: ReadSelection): ReadLocation[] {
  return item.novel ? item.books.find(book => book.key === choice.book)?.locations ?? [] : item.samePaths
}

function forBranch(item: ReadItem, choice: ReadSelection): ReadLocation[] {
  return locations(item, choice).filter(location => location.branch === choice.branch)
}

function setBranchRules(item: ReadItem, choice: ReadSelection) {
  const existing = forBranch(item, choice)
  choice.action = item.novel && choice.branch && (
    (!!item.supersededBy && (local.value || !existing.some(location => location.relation === 'newer')))
    || (!local.value && existing.some(location => location.relation === 'same' || location.relation === 'older'))
  ) ? 'history' : 'keep'
  choice.replaces = choice.action === 'keep' ? suggestedReplacements(item, choice) : []
}

function route(item: ReadItem, choice: ReadSelection) {
  const candidates = [...new Set(locations(item, choice).map(location => location.branch))]
  choice.branch = local.value || !item.novel || candidates.length === 0
    ? props.library.currentBranch
    : candidates.length === 1 ? candidates[0] : ''
  setBranchRules(item, choice)
}

function suggestedReplacements(item: ReadItem, choice: ReadSelection): string[] {
  return forBranch(item, choice).filter(location => item.novel
    ? location.relation === 'newer' || (local.value && location.relation === 'same')
    : location.relation !== 'same' && location.path === choice.destination,
  ).map(location => location.path)
}

function ordinaryStatus(item: ReadItem) {
  const current = item.samePaths.find(location => location.branch === props.library.currentBranch)
  if (!current) return '新增'
  return current.relation === 'same' ? '无变化' : '更新'
}

function toggle(item: ReadItem) {
  if (selected.delete(item.path)) return
  const choice: ReadSelection = {
    path: item.path, book: item.preferredBook, branch: '', action: 'keep',
    destination: item.path, replaces: [],
  }
  route(item, choice)
  selected.set(item.path, choice)
}

function changeAction(item: ReadItem, choice: ReadSelection) {
  choice.replaces = choice.action === 'keep' ? suggestedReplacements(item, choice) : []
}

function confirm(saveExternal: boolean) {
  const active = reading.value
  if (!active) return
  return props.run(() => request(active.intent, saveExternal
    ? { kind: 'saveExternal', token: active.preview.token }
    : { kind: 'apply', token: active.preview.token, selections: [...selected.values()] },
  ))
}
</script>

<template>
  <div class="sheet-body reading-body">
    <template v-if="entry !== 'save'">
      <p v-if="entry === 'scan'">在当前分支递归查找所选格式，预览书目识别和去重结果。</p>
      <p v-else>{{ novelMode ? '选择下载目录，检查书目、格式及目标分支。' : '选择来源目录，按相对路径查看新增和更新。' }}</p>
      <fieldset v-if="novelMode" class="read-formats" :disabled="busy">
        <legend>文件格式</legend>
        <label v-for="format in [...defaultFormats, ...optionalFormats]" :key="format">
          <input v-model="formats" type="checkbox" :value="format" @change="resetPreview" />
          <span>.{{ format }}</span>
        </label>
      </fieldset>
      <button v-if="entry === 'scan'" class="button" type="button" data-initial-focus :disabled="!canScan" @click="scanCurrent">
        <DesktopIcon name="scan" />扫描当前分支
      </button>
      <button v-else class="button" type="button" data-initial-focus :disabled="!canScan" @click="chooseSource">
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
        <label class="search-field read-search">
          <DesktopIcon name="search" />
          <input v-model="search" type="search" aria-label="筛选扫描结果" placeholder="筛选扫描结果" />
        </label>
        <ul class="sheet-list read-results">
          <li v-for="{ item, choice } in visible" :key="item.path" :data-selected="!!choice">
            <label class="read-item">
              <input type="checkbox" :checked="!!choice" :disabled="busy" @change="toggle(item)" />
              <span>
                <strong>{{ item.novel?.title || fileName(item.path) }}</strong>
                <small class="break-text">{{ item.path }}</small>
              </span>
              <span class="format-tag">{{ item.novel?.format.toUpperCase() ?? ordinaryStatus(item) }}</span>
            </label>
            <div v-if="item.novel" class="read-facts">
              <span>{{ item.novel.author || '作者未识别' }}</span>
              <span v-if="item.novel.progress">{{ item.novel.progress }}</span>
              <span v-if="item.novel.completed">完结</span>
              <span v-if="item.novel.revised">修订或特殊版本</span>
              <span v-if="item.novel.supplemental">补充内容</span>
              <span v-if="item.novel.copy !== null">下载副本 {{ item.novel.copy }}</span>
            </div>
            <p v-if="item.supersededBy" class="read-hint">建议保留「{{ item.supersededBy }}」为当前版。</p>
            <div v-if="choice" class="read-choice">
              <label v-if="item.novel" class="field">
                <span>归入书目</span>
                <select v-model="choice.book" :disabled="busy" @change="route(item, choice)">
                  <option :value="null">确认所属书目</option>
                  <option v-for="book in item.books" :key="book.key" :value="book.key">
                    {{ book.title }} · {{ book.author || '作者未识别' }} · {{ book.existing ? '书库候选' : '新书目' }}{{ book.locations.length ? ' · ' + book.locations[0].path : '' }}
                  </option>
                </select>
              </label>
              <label class="field">
                <span>目标分支</span>
                <select v-model="choice.branch" :disabled="busy || local" @change="setBranchRules(item, choice)">
                  <option value="">指定本次更新的分支</option>
                  <option v-for="branch in library.branches" :key="branch">{{ branch }}</option>
                </select>
              </label>
              <label v-if="item.novel" class="field">
                <span>处理方式</span>
                <select v-model="choice.action" :disabled="busy" @change="changeAction(item, choice)">
                  <option value="keep">保留为当前文件</option>
                  <option value="history">保存为旧版</option>
                </select>
              </label>
              <label v-if="!local && choice.action === 'keep'" class="field">
                <span>目标相对路径</span>
                <input v-model="choice.destination" type="text" :disabled="busy" @change="changeAction(item, choice)" />
              </label>
              <fieldset v-if="choice.action === 'keep' && forBranch(item, choice).length" class="read-replacements" :disabled="busy">
                <legend>替换目标文件</legend>
                <label v-for="location in forBranch(item, choice)" :key="location.path">
                  <input v-model="choice.replaces" type="checkbox" :value="location.path" />
                  <span class="break-text">{{ location.path }}</span>
                  <small>{{ relationLabels[location.relation] }}</small>
                </label>
              </fieldset>
            </div>
          </li>
        </ul>
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
