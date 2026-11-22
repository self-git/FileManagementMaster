export type WorkspaceModule = 'files' | 'books'

export const defaultFormats = ['epub', 'txt', 'mobi', 'azw3'] as const
export const optionalFormats = ['pdf', 'azw', 'kfx', 'prc', 'fb2', 'fb2.zip', 'kepub'] as const
export type NovelFormat = (typeof defaultFormats)[number] | (typeof optionalFormats)[number]

export interface NovelName {
  title: string
  author: string | null
  format: NovelFormat
  progress: string | null
  completed: boolean
  revised: boolean
  supplemental: boolean
  copy: number | null
}

export interface Book {
  id: number
  title: string
  author: string | null
  files: string[]
}

export interface Library {
  directory: string
  currentBranch: string
  defaultBranch: string
  branches: string[]
  files: string[]
  books: Book[]
  novels: Partial<Record<string, NovelName>>
  fileDetails: Partial<Record<string, { bytes: number; modifiedAt: number | null }>>
  revision: number
}

export interface NovelScanScope {
  directory: string
  branch: string
  revision: number
  formats: NovelFormat[]
}

export type ScanPhase =
  | { kind: 'discovering'; found: number }
  | { kind: 'identifying'; processed: number; total: number }
  | { kind: 'verifying'; total: number }
  | { kind: 'publishing' }

export type ScanReview = 'unsaved' | 'authorUnclear' | 'authorConflict' | 'bookConflict' | 'historyLimit'

export interface ScannedFile {
  path: string
  novel: NovelName
  saved: boolean
  sameAs: string | null
  supersededBy: string | null
}

export interface ScannedBook {
  id: number
  title: string
  author: string | null
  bookId: number | null
  files: ScannedFile[]
  review: ScanReview | null
}

export interface ScanResult {
  library: Library
  groups: ScannedBook[]
  matched: number
}

export type ScanReviewEdit =
  | { kind: 'merge'; group: number }
  | { kind: 'archive'; group: number; paths: string[] }

export type ScanAction =
  | { kind: 'scan'; scope: NovelScanScope }
  | { kind: 'review'; id: number; edits: ScanReviewEdit[] }

export type ScanEvent =
  | { kind: 'progress'; phase: ScanPhase }
  | { kind: 'ready'; result: ScanResult }
  | { kind: 'stopped' }
  | { kind: 'failed'; message: string }

export type ScanStop = 'requested' | 'publishing' | 'finished'

export type BrowserSelection =
  | { kind: 'files'; paths: string[] }
  | { kind: 'book'; id: number }
  | { kind: 'folder'; path: string }

export function fileName(path: string) {
  return path.slice(path.lastIndexOf('/') + 1)
}

export function fileSuffix(path: string) {
  const leaf = fileName(path)
  const suffix = leaf.toLowerCase()
  if (suffix.endsWith('.fb2.zip')) return 'fb2.zip'
  const dot = suffix.lastIndexOf('.')
  return dot > 0 ? suffix.slice(dot + 1) : ''
}

export function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`
  const unit = bytes >= 1024 ** 3 ? 3 : bytes >= 1024 ** 2 ? 2 : 1
  return `${(bytes / 1024 ** unit).toLocaleString('zh-CN', { maximumFractionDigits: 1 })} ${['', 'KiB', 'MiB', 'GiB'][unit]}`
}

type ChangeKind = 'added' | 'modified' | 'removed' | 'renamed'

export interface ExternalChange {
  kind: ChangeKind
  path: string
  previousPath: string | null
}

export const changeLabels: Record<ChangeKind, string> = {
  added: '新增',
  modified: '修改',
  removed: '删除',
  renamed: '改名',
}
