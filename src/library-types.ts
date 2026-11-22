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
  revision: number
}

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
