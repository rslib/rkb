import { expect, mock, test } from 'claude-code/testing'
import type { Engine } from 'claude-code/testing'
import type { On } from 'claude-code'

type Json = Record<string, unknown>
type Call = { args: string[]; stdin?: string }

const STOP = { session_id: 's1', transcript_path: '/t/s1.jsonl', cwd: '/work/demo', stop_hook_active: false }
const REQUEST = { id: 'r-4f2a9c', question: 'Add this lesson although it is close to 2b81d05e77?', options: ['add anyway', 'cancel'] }

function statusOf(over: Json = {}): Json {
  return { inbox: { total: 31, priority: { '1': 20, '2': 2, '3': 9 } }, curate: 11, requests: [REQUEST], injected: [], ...over }
}

/** A fake rkb: answers each subcommand from `replies`, records every call. */
function fakeRkb(on: On, replies: Record<string, Json | ((c: Call) => Json)>, calls: Call[]): void {
  on('process.run', (_$, e) => {
    const args = [...e.argv.slice(1)]
    const call: Call = { args, stdin: e.init?.stdin }
    calls.push(call)
    const key = args[0] === 'job' ? `job ${args[1]}` : (args[0] ?? '')
    const reply = replies[key]
    const data = typeof reply === 'function' ? reply(call) : reply
    const stdout = data === undefined ? '' : JSON.stringify(data)
    return { value: { exitCode: data === undefined ? 1 : 0, stdout, stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }
  })
}

function world(on: On): { statuses: (string | undefined)[]; toasts: string[] } {
  const seen = { statuses: [] as (string | undefined)[], toasts: [] as string[] }
  on('classic.Stop', () => ({}))
  on('classic.PreCompact', () => ({}))
  on('session.id', () => ({ value: 's1' }))
  on('session.model', () => ({ value: 'claude-opus-5-5' }))
  on('env.get', () => ({ value: undefined }))
  on('ui.render', ($, e) => {
    const { Text } = $.ui.resolve(e)
    return Text({ children: 'engine' })
  })
  on('ui.status', (_$, e) => {
    seen.statuses.push(e.text)
    return { value: undefined }
  })
  on('ui.toast', (_$, e) => {
    seen.toasts.push(e.text)
    return { value: undefined }
  })
  return seen
}

async function stop($: Engine, clock: { settle: () => Promise<void> }): Promise<void> {
  await $.classic.Stop(STOP)
  await clock.settle()
}

test('status line shows waiting work', async ($, on) => {
  const clock = mock.clock(on)
  const seen = world(on)
  const calls: Call[] = []
  fakeRkb(on, { status: statusOf(), 'job prepare': { job: null, skipped: 'not approved' } }, calls)
  await stop($, clock)
  expect(seen.statuses.at(-1)).toBe('31 inbox (9 high) \u00b7 11 curate \u00b7 1 request')
  expect(calls[0]?.args).toEqual(['status', '--session', 's1', '--format', 'json'])
})

test('status line is empty when nothing waits', async ($, on) => {
  const clock = mock.clock(on)
  const seen = world(on)
  fakeRkb(on, { status: statusOf({ inbox: { total: 0, priority: {} }, curate: 0, requests: [] }), 'job prepare': { job: null } }, [])
  await stop($, clock)
  expect(seen.statuses.at(-1)).toBe(undefined)
})

test('a failing rkb leaves the session alone', async ($, on) => {
  const clock = mock.clock(on)
  const seen = world(on)
  let n = 0
  on('process.run', () => {
    n += 1
    if (n === 1) return { deny: 'rkb is not installed' }
    return { value: { exitCode: 0, stdout: 'not json', stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }
  })
  await expect($.classic.Stop(STOP)).resolves.toEqual({})
  await clock.settle()
  await $.classic.Stop(STOP)
  await clock.settle()
  expect(seen.statuses).toEqual([])
  expect(seen.toasts).toEqual([])
})

for (const surface of ['terminal', 'desktop'] as const) {
  test(`band shows injected lessons and reports one irrelevant (${surface})`, async ($, on) => {
    const clock = mock.clock(on)
    world(on)
    const calls: Call[] = []
    const injected = [{ id: '7f3a9c2b41', title: 'CMake cannot find HDF5 unless HDF5_ROOT is set', hook: 'recall', time: 1 }]
    fakeRkb(on, { status: statusOf({ injected }), 'job prepare': { job: null }, used: { recorded: true } }, calls)
    await stop($, clock)
    const band = await $.ui.mount({
      plugin: 'rkb',
      surface,
      component: 'AbovePrompt',
      props: { hasSurvey: false, isWorking: false, maxRows: 10, bodyColumns: 100, scroll: { offset: 0, bodyRows: 10 }, view: {} },
    })
    const line = await band.find({ key: 'lesson-7f3a9c2b41' })
    for (const part of ['recall', 'CMake cannot find HDF5 unless HDF5_ROOT is set', '7f3a9c2b41']) expect(line?.text).toContain(part)
    await band.press({ key: 'irrelevant-7f3a9c2b41' })
    expect(calls.some(c => c.args.join(' ') === 'used --irrelevant 7f3a9c2b41 --session s1 --format json')).toBe(true)
    expect(await band.find({ key: 'lesson-7f3a9c2b41' })).toBe(undefined)
    await stop($, clock)
    expect(await band.find({ key: 'lesson-7f3a9c2b41' })).toBe(undefined)
  })
}

function pane($: Engine) {
  return $.ui.mount({
    plugin: 'rkb',
    surface: 'terminal',
    component: 'Pane',
    requestId: 'rkb',
    props: { title: 'rkb', isFocused: true, bodyColumns: 100, placement: 'dock', scroll: { offset: 0, bodyRows: 30 }, view: {} },
  })
}

test('search from the pane adds nothing to the conversation', async ($, on) => {
  const clock = mock.clock(on)
  world(on)
  let prompts = 0
  on('prompt.submit', (_$, e) => {
    prompts += 1
    return { text: e.text }
  })
  const results = [{ id: '1a00000012', title: 'Undefined reference to vtable means a missing virtual definition', relevance: '0.91' }]
  fakeRkb(on, { status: statusOf(), 'job prepare': { job: null }, search: { results } }, [])
  await stop($, clock)
  const ui = await pane($)
  await ui.input({ key: 'query', text: 'undefined reference to vtable' })
  expect((await ui.find({ key: 'hit-1a00000012' }))?.text).toContain('Undefined reference to vtable')
  expect((await ui.find({ key: 'result-1a00000012' }))?.text).toContain('1a00000012')
  expect(prompts).toBe(0)
})

function answering(on: On, choice: string | undefined): void {
  on('tool.call', { tool: 'AskUserQuestion' }, (_$, e) => {
    if (choice === undefined) return { deny: 'dismissed' }
    const question = e.questions[0]?.question ?? ''
    return { result: { questions: e.questions, answers: { [question]: choice } } }
  })
}

test('a request is answered from the pane', async ($, on) => {
  const clock = mock.clock(on)
  const seen = world(on)
  answering(on, 'add anyway')
  const calls: Call[] = []
  let answered = false
  fakeRkb(
    on,
    {
      status: () => statusOf({ requests: answered ? [] : [REQUEST] }),
      'job prepare': { job: null },
      confirm: () => {
        answered = true
        return { status: 'done', id: 'c04e11a9f3', message: 'added' }
      },
    },
    calls,
  )
  await stop($, clock)
  const ui = await pane($)
  await ui.press({ key: 'view-requests' })
  await ui.press({ key: 'answer-r-4f2a9c' })
  expect(calls.some(c => c.args.join(' ') === 'confirm r-4f2a9c --choice add anyway --format json')).toBe(true)
  expect(seen.toasts.at(-1)).toContain('c04e11a9f3')
  expect(await ui.find({ key: 'request-r-4f2a9c' })).toBe(undefined)
})

test('a dismissed dialog confirms nothing', async ($, on) => {
  const clock = mock.clock(on)
  world(on)
  answering(on, undefined)
  const calls: Call[] = []
  fakeRkb(on, { status: statusOf(), 'job prepare': { job: null } }, calls)
  await stop($, clock)
  const ui = await pane($)
  await ui.press({ key: 'view-requests' })
  await ui.press({ key: 'answer-r-4f2a9c' })
  expect(calls.some(c => c.args[0] === 'confirm')).toBe(false)
  expect(await ui.find({ key: 'request-r-4f2a9c' })).not.toBe(undefined)
})

test('the observer runs a job after a turn', async ($, on) => {
  const clock = mock.clock(on)
  const seen = world(on)
  const asked: string[] = []
  on('model.complete', (_$, e) => {
    asked.push(e.model)
    const usage = { input_tokens: 1, output_tokens: 1, cache_creation_input_tokens: 0, cache_read_input_tokens: 0 }
    return { value: { isAnswered: true, text: '- 2026-10-02 [high] (pitfall; general) note', usage } }
  })
  const calls: Call[] = []
  const job = { job: 'j-0a1b2c3d', kind: 'observe', models: [{ entry: 'sonnet', model: 'sonnet' }], part: 0, parts: 1, prompt: 'the prompt' }
  fakeRkb(on, { status: statusOf(), 'job prepare': job, 'job finish': { outcome: 'added', id: 'abc', notes: 1, dropped: 0, via: 'sonnet' } }, calls)
  await stop($, clock)
  expect(asked).toEqual(['sonnet'])
  const finish = calls.find(c => c.args[1] === 'finish')
  expect(finish?.args).toEqual(['job', 'finish', 'j-0a1b2c3d', '--part', '0', '--model', 'sonnet', '--format', 'json'])
  expect(finish?.stdin).toBe('- 2026-10-02 [high] (pitfall; general) note')
  const prepare = calls.find(c => c.args[1] === 'prepare')
  expect(prepare?.args).toContain('/t/s1.jsonl')
  expect(seen.toasts).toContain('rkb: 1 note for the inbox')
})

test('the observer runs no model without an approved job', async ($, on) => {
  const clock = mock.clock(on)
  world(on)
  let models = 0
  on('model.complete', () => {
    models += 1
    return { deny: 'not expected' }
  })
  fakeRkb(on, { status: statusOf(), 'job prepare': { job: null, skipped: 'the observer is not approved here' } }, [])
  await stop($, clock)
  expect(models).toBe(0)
})
