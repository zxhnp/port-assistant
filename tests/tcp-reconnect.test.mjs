import assert from 'node:assert/strict'
import { after, before, test } from 'node:test'
import { fileURLToPath } from 'node:url'
import { createServer } from 'vite'
import { createRenderer, h, nextTick } from 'vue'

const projectRoot = fileURLToPath(new URL('../', import.meta.url))
let server
let useWorkbench
let transport
let originalWindow

const renderer = createRenderer({
  createElement: type => ({ type, children: [] }),
  createText: text => ({ text }),
  createComment: text => ({ text }),
  setText: (node, text) => { node.text = text },
  setElementText: (node, text) => { node.text = text },
  parentNode: () => null,
  nextSibling: () => null,
  insert: (node, parent) => { parent.children.push(node) },
  remove: () => {},
  patchProp: () => {},
})

const wait = ms => new Promise(resolve => setTimeout(resolve, ms))
const address = { kind: 'tcpClient', remote: { host: '127.0.0.1', port: 19381 } }

before(async () => {
  originalWindow = globalThis.window
  globalThis.window = globalThis
  server = await createServer({
    configFile: false,
    root: projectRoot,
    resolve: { alias: { '@': fileURLToPath(new URL('../src', import.meta.url)) } },
    server: { middlewareMode: true },
    appType: 'custom',
    logLevel: 'silent',
  })
  ;({ useWorkbench } = await server.ssrLoadModule('/src/composables/useWorkbench.ts'))
  ;({ transport } = await server.ssrLoadModule('/src/lib/transport.ts'))
  const { Message } = await server.ssrLoadModule('/src/lib/message.ts')
  Object.assign(Message, { error() {}, success() {}, dismiss() {} })
})

after(async () => {
  await server?.close()
  if (originalWindow === undefined) delete globalThis.window
  else globalThis.window = originalWindow
})

function createWorkbench() {
  let state
  const app = renderer.createApp({ setup() { state = useWorkbench(); return () => h('div') } })
  app.mount({ children: [] })
  return { state, unmount: () => app.unmount() }
}

test('TCP 自动重连在失败、恢复、断线和取消时保持状态一致', async () => {
  const original = { ...transport }
  let callback
  let attempts = 0
  let pending
  let blockAttempt = false
  Object.assign(transport, {
    async connect(_connection, _framing, events) {
      attempts++
      callback = events
      if (attempts === 1) throw new Error('连接失败')
      if (blockAttempt) await new Promise(resolve => { pending = resolve })
      events({ type: 'status', connected: true, endpoint: '127.0.0.1:19381', clients: [] })
    },
    async disconnect() { pending?.(); pending = undefined },
    async setFraming() {},
  })
  const { state, unmount } = createWorkbench()
  try {
    state.autoReconnect.value = true
    state.reconnectIntervalMs.value = 100
    await state.connect(address)
    assert.equal(state.reconnecting.value, true)
    await wait(250)
    assert.equal(state.connected.value, true)
    assert.equal(attempts, 2)

    state.running.value = true
    callback({ type: 'status', connected: false, endpoint: '', clients: [] })
    assert.equal(state.reconnecting.value, true)
    assert.equal(state.running.value, false)
    await wait(250)
    assert.equal(state.connected.value, true)
    assert.equal(state.running.value, false)

    blockAttempt = true
    callback({ type: 'status', connected: false, endpoint: '', clients: [] })
    await wait(150)
    assert.equal(state.busy.value, true)
    await state.disconnect()
    await nextTick()
    await wait(150)
    assert.equal(state.connected.value, false)
    assert.equal(state.reconnecting.value, false)
    const stopped = attempts
    await wait(150)
    assert.equal(attempts, stopped)
  } finally {
    unmount()
    Object.assign(transport, original)
  }
})

test('默认关闭自动重连时连接失败仅尝试一次', async () => {
  const original = { ...transport }
  let attempts = 0
  Object.assign(transport, {
    async connect() { attempts++; throw new Error('连接失败') },
    async disconnect() {},
  })
  const { state, unmount } = createWorkbench()
  try {
    state.reconnectIntervalMs.value = 100
    await state.connect(address)
    await wait(200)
    assert.equal(state.autoReconnect.value, false)
    assert.equal(state.reconnecting.value, false)
    assert.equal(attempts, 1)
  } finally {
    unmount()
    Object.assign(transport, original)
  }
})
