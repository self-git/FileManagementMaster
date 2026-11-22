<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue'
import DesktopIcon from './DesktopIcon.vue'
import { defaultFormats, optionalFormats, type NovelFormat, type ScannedBook, type ScanResult, type ScanReview, type ScanReviewEdit } from './library-types'

const props = defineProps<{
  branch: string
  formats: NovelFormat[]
  result: ScanResult | null
}>()
const emit = defineEmits<{
  close: []
  start: [formats: NovelFormat[]]
  review: [edits: ScanReviewEdit[]]
  saveExternal: []
}>()

const allFormats = [...defaultFormats, ...optionalFormats]
const selectedFormats = ref<NovelFormat[]>([...props.formats])
const search = ref('')
const pendingOnly = ref(false)
const page = ref(1)
const pageSize = 24
const expanded = ref<number | null>(null)
// 以书目和路径保存选择，搜索与翻页只改变可见行。
const archivePaths = reactive(new Map<number, Set<string>>())
const confirmedMerge = ref<ScannedBook | null>(null)
const confirmArchive = ref(false)

const reviewLabels = {
  unsaved: '有未保存的文件改动',
  authorUnclear: '书名或作者待确认',
  authorConflict: '作者信息冲突',
  bookConflict: '已分别归入不同书目',
  historyLimit: '归组会触发历史保留上限',
} satisfies Record<ScanReview, string>

const groups = computed(() => props.result?.groups ?? [])
const pending = computed(() => groups.value.filter(group => group.review !== null).length)
const hasUnsaved = computed(() => groups.value.some(group => group.review === 'unsaved'))
const filtered = computed(() => {
  const term = search.value.trim().toLocaleLowerCase()
  return groups.value.filter(group => {
    if (pendingOnly.value && !group.review) return false
    return !term || group.title.toLocaleLowerCase().includes(term)
      || group.author?.toLocaleLowerCase().includes(term)
      || group.files.some(file => file.path.toLocaleLowerCase().includes(term))
  })
})
const pageCount = computed(() => Math.max(1, Math.ceil(filtered.value.length / pageSize)))
const shown = computed(() => filtered.value.slice((page.value - 1) * pageSize, page.value * pageSize))
const archivedCount = computed(() => [...archivePaths.values()].reduce((sum, paths) => sum + paths.size, 0))
const archiveEdits = computed(() => [...archivePaths].flatMap(([group, paths]) =>
  paths.size ? [{ kind: 'archive', group, paths: [...paths] } satisfies ScanReviewEdit] : [],
))

watch([search, pendingOnly], () => { page.value = 1 })

function toggleFormats() {
  selectedFormats.value = selectedFormats.value.length === allFormats.length ? [] : [...allFormats]
}

function canArchive(group: ScannedBook, path: string, format: NovelFormat) {
  const paths = archivePaths.get(group.id)
  return paths?.has(path) || group.files.some(file =>
    file.path !== path && file.novel.format === format && !paths?.has(file.path),
  )
}

function toggleArchive(group: ScannedBook, path: string) {
  const paths = new Set(archivePaths.get(group.id))
  if (!paths.delete(path)) paths.add(path)
  if (paths.size) archivePaths.set(group.id, paths)
  else archivePaths.delete(group.id)
}
</script>

<template>
  <template v-if="!result">
    <div class="sheet-body">
      <p>在「{{ branch }}」递归查找小说，并自动归入书目。各版本的原始文件会保留。</p>
      <fieldset class="read-formats">
        <legend>文件格式</legend>
        <button class="text-button" type="button" @click="toggleFormats">
          {{ selectedFormats.length === allFormats.length ? '取消全选格式' : '全选格式' }}
        </button>
        <label v-for="format in allFormats" :key="format">
          <input v-model="selectedFormats" type="checkbox" :value="format" />
          <span>.{{ format }}</span>
        </label>
      </fieldset>
    </div>
    <footer class="sheet-footer">
      <button class="button" type="button" @click="$emit('close')">关闭</button>
      <button class="button primary" type="button" data-initial-focus :disabled="!selectedFormats.length"
        @click="$emit('start', selectedFormats)"><DesktopIcon name="scan" />开始扫描并归组</button>
    </footer>
  </template>
  <template v-else>
    <div class="sheet-body">
      <p v-if="confirmedMerge">将「{{ confirmedMerge.title }}」归入同一本书目。
        归组后按每种格式保留最近 10 个历史版本，所有当前文件继续保留。</p>
      <p v-else-if="confirmArchive">将选中的 {{ archivedCount }} 个文件移入对应格式的版本历史。</p>
      <template v-else>
        <p>「{{ branch }}」找到 {{ result.matched }} 个文件 · {{ pending }} 组待确认</p>
        <div v-if="groups.length" class="read-tools">
          <label class="search-field read-search">
            <DesktopIcon name="search" />
            <input v-model="search" type="search" data-initial-focus aria-label="搜索扫描结果" placeholder="搜索书名、作者或路径" />
          </label>
          <label v-if="pending" class="check-field"><input v-model="pendingOnly" type="checkbox" />仅看待确认</label>
          <button v-if="hasUnsaved" class="button" type="button" @click="$emit('saveExternal')">保存外部改动</button>
        </div>
        <ul v-if="shown.length" class="sheet-list scan-groups" aria-label="扫描结果书目">
          <li v-for="group in shown" :key="group.id">
            <button type="button" class="scan-group-toggle" :aria-expanded="expanded === group.id"
              @click="expanded = expanded === group.id ? null : group.id">
              <DesktopIcon :name="expanded === group.id ? 'down' : 'right'" />
              <span class="break-text">{{ group.title }}<small v-if="group.author"> · {{ group.author }}</small></span>
              <small>{{ group.files.length }} 个文件</small>
            </button>
            <p v-if="group.review" class="scan-review-note">{{ reviewLabels[group.review] }}</p>
            <div v-if="expanded === group.id" class="scan-versions">
              <ul class="sheet-list" :aria-label="group.title + ' 的版本'">
                <li v-for="file in group.files" :key="file.path">
                  <span class="format-tag">{{ file.novel.format.toUpperCase() }}</span>
                  <div class="scan-version-path">
                    <strong class="break-text">{{ file.path }}</strong>
                    <small v-if="!file.saved">请先保存文件改动</small>
                    <small v-else-if="file.sameAs" class="break-text">内容相同 · {{ file.sameAs }}</small>
                    <small v-else-if="file.supersededBy" class="break-text">覆盖更完整 · {{ file.supersededBy }}</small>
                    <small v-else-if="file.novel.progress || file.novel.completed">
                      {{ file.novel.progress || '完结' }}
                    </small>
                  </div>
                  <label v-if="group.bookId !== null && !group.review" class="check-field">
                    <input type="checkbox" :aria-label="'将 ' + file.path + ' 移入历史'"
                      :checked="archivePaths.get(group.id)?.has(file.path) ?? false"
                      :disabled="!canArchive(group, file.path, file.novel.format)"
                      @change="toggleArchive(group, file.path)" />
                    移入历史
                  </label>
                </li>
              </ul>
              <button v-if="group.review === 'bookConflict' || group.review === 'historyLimit'" type="button"
                class="button" @click="confirmedMerge = group">确认归入同一书目</button>
            </div>
          </li>
        </ul>
        <p v-else>{{ groups.length ? '没有符合筛选条件的书目。' : '此分支没有匹配所选格式的文件。' }}</p>
        <nav v-if="pageCount > 1" class="scan-pagination" aria-label="扫描结果分页">
          <button class="button" type="button" :disabled="page === 1" @click="page--">上一页</button>
          <span>{{ page }} / {{ pageCount }}</span>
          <button class="button" type="button" :disabled="page === pageCount" @click="page++">下一页</button>
        </nav>
      </template>
    </div>
    <footer class="sheet-footer">
      <template v-if="confirmedMerge">
        <button class="button" type="button" @click="confirmedMerge = null">返回结果</button>
        <button class="button primary" type="button" @click="$emit('review', [{ kind: 'merge', group: confirmedMerge.id }])">归组</button>
      </template>
      <template v-else-if="confirmArchive">
        <button class="button" type="button" @click="confirmArchive = false">返回结果</button>
        <button class="button primary" type="button" @click="$emit('review', archiveEdits)">确认移入历史</button>
      </template>
      <template v-else>
        <button class="button" type="button" @click="$emit('close')">关闭</button>
        <button v-if="archivedCount" class="button primary" type="button" @click="confirmArchive = true">
          移入历史 · {{ archivedCount }}
        </button>
      </template>
    </footer>
  </template>
</template>
