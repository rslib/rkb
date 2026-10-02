export type Injected = { id: string; title: string | null; hook: string | null }
export type Request = { id: string; question: string; options: string[] }
export type Status = { inbox: number; high: number; curate: number; requests: Request[] }
export type Hit = { id: string; title: string; relevance: string | null; kind: string; summary: string }
export type Lesson = { id: string; title: string; body: string; meta: string }
export type Item = { id: string; title: string; body: string; meta: string }
export type InboxRow = { id: string; kind: string; priority: number; age: string; preview: string }
export type View = 'search' | 'lesson' | 'inbox' | 'item' | 'requests'

declare module 'claude-code' {
  interface PluginState {
    rkb: {
      status: Status | null
      band: Injected[]
      seen: string[]
      view: View
      hits: Hit[]
      lesson: Lesson | null
      item: Item | null
      inbox: InboxRow[]
      note: string
    }
  }
}
