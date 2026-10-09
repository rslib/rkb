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
function fakeRkb(on: On, replies: Record<string, Json | string | ((c: Call) => Json | string)>, calls: Call[]): void {
  on('process.run', (_$, e) => {
    const args = [...e.argv.slice(1)]
    const call: Call = { args, stdin: e.init?.stdin }
    calls.push(call)
    const key = args[0] === 'job' || args[0] === 'hook' ? `${args[0]} ${args[1]}` : (args[0] ?? '')
    const reply = replies[key]
    const data = typeof reply === 'function' ? reply(call) : reply
    const stdout = data === undefined ? '' : typeof data === 'string' ? data : JSON.stringify(data)
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
  expect(calls.find(c => c.args[0] === 'status')?.args).toEqual(['status', '--session', 's1', '--format', 'json'])
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
  const card = (await ui.find({ key: 'result-1a00000012' }))?.text
  for (const part of ['Undefined reference to vtable', '1a00000012', '0.91']) expect(card).toContain(part)
  expect((await ui.find({ key: 'hit-1a00000012' }))?.text).toContain('Open')
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
  expect((await ui.find({ key: 'notice' }))?.text).toContain('c04e11a9f3')
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

test('recall reaches the prompt through the mod', async ($, on) => {
  world(on)
  on('classic.UserPromptSubmit', () => ({ additionalContext: ['beneath'] }))
  const calls: Call[] = []
  const reply = { hookSpecificOutput: { hookEventName: 'UserPromptSubmit', additionalContext: 'rkb: a lesson' } }
  fakeRkb(on, { 'hook prompt': reply }, calls)
  const r = await $.classic.UserPromptSubmit({ ...STOP, prompt: 'why does cmake not find hdf5' })
  expect(r.additionalContext).toEqual(['beneath', 'rkb: a lesson'])
  const call = calls.find(c => c.args[0] === 'hook')
  expect(call?.args).toEqual(['hook', 'prompt', '--mod'])
  expect(JSON.parse(call?.stdin ?? '{}').prompt).toBe('why does cmake not find hdf5')
})

test('session start names the session in RKB_MOD and its context goes with the first prompt', async ($, on) => {
  world(on)
  const env: Record<string, string | undefined> = {}
  on('env.set', (_$, e) => {
    env[e.name] = e.value
    return { value: undefined }
  })
  let seenByCommandHooks: string | undefined
  on('classic.SessionStart', () => {
    seenByCommandHooks = env.RKB_MOD
    return {}
  })
  on('classic.UserPromptSubmit', () => ({}))
  const clock = mock.clock(on)
  fakeRkb(on, { 'hook session-start': 'rkb: no project, no system', 'hook prompt': '' }, [])
  const r = await $.classic.SessionStart({ ...STOP, source: 'startup' })
  expect(seenByCommandHooks).toBe('s1')
  expect(r.additionalContext).toBe(undefined)
  await clock.settle()
  const first = await $.classic.UserPromptSubmit({ ...STOP, prompt: 'hi' })
  expect(first.additionalContext).toEqual(['rkb: no project, no system'])
  const second = await $.classic.UserPromptSubmit({ ...STOP, prompt: 'again' })
  expect(second.additionalContext).toBe(undefined)
})

test('a stop reply that blocks keeps the reason', async ($, on) => {
  const clock = mock.clock(on)
  world(on)
  fakeRkb(on, { 'hook stop': { decision: 'block', reason: 'Record the lesson now.' }, status: statusOf(), 'job prepare': { job: null } }, [])
  const r = await $.classic.Stop(STOP)
  await clock.settle()
  expect(r.block).toBe('Record the lesson now.')
})

test('only Bash tool results go to rkb', async ($, on) => {
  world(on)
  on('classic.PostToolUse', () => ({}))
  const calls: Call[] = []
  fakeRkb(on, { 'hook tool-ok': '' }, calls)
  await $.classic.PostToolUse({ ...STOP, tool_name: 'Read', tool_input: {}, tool_response: '', tool_use_id: 't1' })
  expect(calls).toEqual([])
  await $.classic.PostToolUse({ ...STOP, tool_name: 'Bash', tool_input: { command: 'ls' }, tool_response: '', tool_use_id: 't2' })
  expect(calls.map(c => c.args.slice(0, 2))).toEqual([['hook', 'tool-ok']])
})

const TOOLS = {
  tools: ['rkb_search', 'rkb_show', 'rkb_add', 'rkb_note', 'rkb_used', 'rkb_flag', 'rkb_edit'].map(name => ({
    name,
    description: `${name} description`,
    inputSchema: { type: 'object', properties: {} },
  })),
}

test('the tools register under short names and none confirms', async ($, on) => {
  world(on)
  const names: string[] = []
  on('tool.register', (_$, e) => {
    names.push(e.name)
    return { value: { name: e.name } } as never
  })
  on('command.register', (_$, e) => ({ value: { command: e.name } }) as never)
  on('session.start', (_$, e) => ({ cwd: e.cwd }))
  const clock = mock.clock(on)
  fakeRkb(on, { tools: TOOLS }, [])
  await $.session.start({ cwd: '/work/demo', surface: 'terminal', isInteractive: true })
  await clock.settle()
  expect(names).toEqual(['search', 'show', 'add', 'note', 'used', 'flag', 'edit'])
  expect(names.some(n => n.includes('confirm'))).toBe(false)
})

test('a tool call runs rkb tool', async ($, on) => {
  world(on)
  on('tool.register', (_$, e) => ({ value: { name: e.name } }) as never)
  on('command.register', (_$, e) => ({ value: { command: e.name } }) as never)
  on('session.start', (_$, e) => ({ cwd: e.cwd }))
  const clock = mock.clock(on)
  const calls: Call[] = []
  fakeRkb(on, { tools: TOOLS, tool: (c: Call) => (c.args[1] === 'rkb_search' ? 'results[1]{id}:\n  7f3a9c2b41' : '') }, calls)
  await $.session.start({ cwd: '/work/demo', surface: 'terminal', isInteractive: true })
  await clock.settle()
  calls.length = 0
  const r = await $.tool.call({ tool: 'mcp__rkb__search', tool_use_id: 't1', query: 'undefined reference to vtable' } as never)
  expect(r).toMatchObject({ result: 'results[1]{id}:\n  7f3a9c2b41' })
  expect(calls[0]?.args).toEqual(['tool', 'rkb_search'])
  expect(JSON.parse(calls[0]?.stdin ?? '{}')).toEqual({ query: 'undefined reference to vtable' })
})

test('search is allowed and edit still asks', async ($, on) => {
  world(on)
  on('tool.check', () => ({ decision: 'ask' as const }))
  expect((await $.tool.check({ tool: 'mcp__rkb__search', input: {} })).decision).toBe('allow')
  expect((await $.tool.check({ tool: 'mcp__rkb__edit', input: {} })).decision).toBe('ask')
})

test('the inbox actions queue the plugin commands', async ($, on) => {
  const clock = mock.clock(on)
  world(on)
  const ran: string[] = []
  on('command.run', (_$, e) => {
    ran.push(e.command)
    return {}
  })
  fakeRkb(on, { status: statusOf(), 'job prepare': { job: null }, inbox: { items: [] } }, [])
  await stop($, clock)
  const ui = await pane($)
  await ui.press({ key: 'view-inbox' })
  for (const key of ['distill', 'curate', 'retro']) await ui.press({ key })
  await clock.settle()
  expect(ran).toEqual(['rkb:distill', 'rkb:curate', 'rkb:retro'])
  expect((await ui.find({ key: 'notice' }))?.text).toContain('Queued /rkb:retro')
})


for (const surface of ['terminal', 'desktop'] as const) {
  test(`the inbox shows verdicts and triages from the pane (${surface})`, async ($, on) => {
    const clock = mock.clock(on)
    world(on)
    const calls: Call[] = []
    let triaged = false
    const row = (verdicts: Json | undefined) => ({ id: 'aaaaaaaaaa', kind: 'observed', priority: 3, age: '2 h', preview: 'a note', ...(verdicts ? { verdicts } : {}) })
    fakeRkb(
      on,
      {
        status: statusOf(),
        'job prepare': { job: null },
        inbox: (c: Call) =>
          c.args[1] === 'triage'
            ? ((triaged = true), { items: [], total: { keep: 2, known: 0, unsure: 0, drop: 1 } })
            : { items: [row(triaged ? { keep: 2, known: 0, unsure: 0, drop: 1 } : undefined)] },
      },
      calls,
    )
    await stop($, clock)
    const ui = await $.ui.mount({
      plugin: 'rkb',
      surface,
      component: 'Pane',
      requestId: 'rkb',
      props: { title: 'rkb', isFocused: true, bodyColumns: 100, placement: 'dock', scroll: { offset: 0, bodyRows: 30 }, view: {} },
    })
    await ui.press({ key: 'view-inbox' })
    const triages = () => calls.filter(c => c.args.slice(0, 2).join(' ') === 'inbox triage').length
    const before = triages()
    expect(before).toBe(0)
    await ui.press({ key: 'triage' })
    expect(triages()).toBe(before + 1)
    expect((await ui.find({ key: 'item-aaaaaaaaaa' }))?.text).toContain('2 keep')
    expect((await ui.find({ key: 'notice' }))?.text).toContain('2 keep, 0 known, 0 unsure, 1 drop')
  })
}

test('a long inbox item with control characters still renders', async ($, on) => {
  const clock = mock.clock(on)
  world(on)
  const calls: Call[] = []
  const row = { id: 'bbbbbbbbbb', kind: 'observed', priority: 3, age: '2 h', preview: 'a long extract' }
  const body = `start\r\n\u001b[31mred\n${'line of text\n'.repeat(1600)}`
  fakeRkb(on, { status: statusOf(), 'job prepare': { job: null }, inbox: (c: Call) => (c.args[1] === 'show' ? { body, meta: {} } : { items: [row] }) }, calls)
  await stop($, clock)
  const ui = await $.ui.mount({
    plugin: 'rkb',
    surface: 'terminal',
    component: 'Pane',
    requestId: 'rkb',
    props: { title: 'rkb', isFocused: true, bodyColumns: 100, placement: 'dock', scroll: { offset: 0, bodyRows: 30 }, view: {} },
  })
  await ui.press({ key: 'view-inbox' })
  await ui.press({ key: 'open-bbbbbbbbbb' })
  expect((await ui.find({ key: 'item' }))?.text).toContain('more characters. Run `rkb inbox show bbbbbbbbbb`')
})



test('/rkb-status says why the pane did not open', async ($, on) => {
  world(on)
  on('ui.open', () => ({ value: { isPlaced: false as const, reason: 'terminal too narrow' } }))
  expect(await $.command.run({ command: 'rkb-status', args: '', origin: { kind: 'composer' }, presentation: { isFullscreen: true, columns: 100 } })).toMatchObject({ text: 'rkb: the pane did not open: terminal too narrow' })
})

