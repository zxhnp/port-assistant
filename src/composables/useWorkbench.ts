import { computed, onBeforeUnmount, shallowRef, watch } from 'vue'
import { displayText, formatFrameText, formatHex, makePacket, parseFrameText, parseFraming, parseHex } from '@/lib/bytes'
import type { Framing } from '@/lib/bytes'
import { Message, messageText } from '@/lib/message'
import { transport } from '@/lib/transport'
import type { Connection, Peer, Target, TransportEvent, WireRecord } from '@/lib/transport'

export interface ConnectionIndicator {
  tone: 'online' | 'failed' | 'offline'
  label: '已连接' | '已监听' | '模拟连接' | '模拟监听' | '连接失败' | '未连接' | '重连中'
}

export interface LogRow extends Omit<WireRecord, 'packet'> {
  id: number
  time: string
  packet: { bytes: Uint8Array; headerLen: number; tailLen: number }
  hex: string
  text: string
}

export interface TrafficStats {
  tx: { records: number; bytes: number }
  rx: { records: number; bytes: number }
}

function emptyTrafficStats(): TrafficStats {
  return { tx: { records: 0, bytes: 0 }, rx: { records: 0, bytes: 0 } }
}

const MAX_ROWS = 2000
const MAX_LOG_BYTES = 8 * 1024 * 1024
const timeFormat = new Intl.DateTimeFormat('sv-SE', {
  year: 'numeric', month: '2-digit', day: '2-digit',
  hour: '2-digit', minute: '2-digit', second: '2-digit', hourCycle: 'h23',
})

export function useWorkbench() {
  const connected = shallowRef(false)
  const connectionFailed = shallowRef(false)
  const reconnecting = shallowRef(false)
  const autoReconnect = shallowRef(false)
  const reconnectIntervalMs = shallowRef<number | undefined>(3000)
  const activeConnection = shallowRef<Connection | null>(null)
  const endpoint = shallowRef('')
  const clients = shallowRef<Peer[]>([])
  const ports = shallowRef<string[]>([])
  const busy = shallowRef(false)
  const refreshing = shallowRef(false)
  const sending = shallowRef(false)
  const timerBusy = shallowRef(false)
  const running = shallowRef(false)
  const pinned = shallowRef(false)
  const pinBusy = shallowRef(false)

  const target = shallowRef<Target | null>(null)
  const input = shallowRef('')
  const sendMode = shallowRef<'text' | 'hex'>('text')
  const timed = shallowRef(false)
  const intervalMs = shallowRef<number | undefined>(1000)
  const addFrame = shallowRef(false)
  const splitFrames = shallowRef(false)
  const frameMode = shallowRef<'hex' | 'text'>('text')
  const headerInput = shallowRef('\\u0001')
  const tailInput = shallowRef('\\u0004')

  function setFrameMode(mode: 'hex' | 'text') {
    if (mode === frameMode.value) return
    const convert = mode === 'hex'
      ? (value: string) => formatHex(parseFrameText(value))
      : (value: string) => formatFrameText(parseHex(value, 1024))
    let header: string
    let tail: string
    try {
      header = convert(headerInput.value)
    } catch (cause) {
      Message.error(`帧头${messageText(cause)}`, 'framing')
      return
    }
    try {
      tail = convert(tailInput.value)
    } catch (cause) {
      Message.error(`帧尾${messageText(cause)}`, 'framing')
      return
    }
    headerInput.value = header
    tailInput.value = tail
    frameMode.value = mode
    Message.dismiss('framing')
  }

  const paused = shallowRef(false)
  const visible = shallowRef<LogRow[]>([])
  const trafficStats = shallowRef<TrafficStats>(emptyTrafficStats())
  const buffered: LogRow[] = []
  let bufferedBytes = 0
  let nextRow = 1
  let animation: number | undefined
  let generation = 0
  let reconnectTimer: number | undefined
  let retryConnection: Connection | null = null

  const framing = computed(() => {
    try {
      return { value: parseFraming(headerInput.value, tailInput.value, frameMode.value), error: '' }
    } catch (cause) {
      return { value: null, error: messageText(cause) }
    }
  })

  const packet = computed(() => {
    if (addFrame.value && !framing.value.value) return { value: null, error: framing.value.error }
    try {
      return { value: makePacket(input.value, sendMode.value, addFrame.value ? framing.value.value : null), error: '' }
    } catch (cause) {
      return { value: null, error: messageText(cause) }
    }
  })
  const interval = computed(() => Number(intervalMs.value))
  const validInterval = computed(() => Number.isInteger(interval.value) && interval.value >= 1 && interval.value <= 2_147_483_647)

  function hasTarget(connection: Connection | null) {
    const destination = target.value
    if (connection?.kind === 'udp') {
      return destination?.kind === 'udp' && !!destination.host && Number.isInteger(destination.port)
        && destination.port >= 1 && destination.port <= 65_535
    }
    if (connection?.kind === 'tcpServer') {
      return destination?.kind === 'peer' && clients.value.some(peer => peer.id === destination.id)
    }
    return true
  }

  const connectionIndicator = computed<ConnectionIndicator>(() => {
    if (!connected.value) {
      if (reconnecting.value) return { tone: 'offline', label: '重连中' }
      return connectionFailed.value
        ? { tone: 'failed', label: '连接失败' }
        : { tone: 'offline', label: '未连接' }
    }
    const kind = activeConnection.value?.kind
    const listening = kind === 'udp' || kind === 'tcpServer'
    if (transport.simulated) {
      return { tone: 'online', label: listening ? '模拟监听' : '模拟连接' }
    }
    return { tone: 'online', label: listening ? '已监听' : '已连接' }
  })

  const canSend = computed(() => connected.value && !busy.value && !!packet.value.value?.bytes.length
    && hasTarget(activeConnection.value))

  function syncDisplay() {
    if (animation !== undefined) return
    animation = window.requestAnimationFrame(() => {
      animation = undefined
      if (!paused.value) visible.value = buffered.slice()
    })
  }

  function append(received: WireRecord[]) {
    const stats = { tx: { ...trafficStats.value.tx }, rx: { ...trafficStats.value.rx } }
    for (const { packet, ...record } of received) {
      const bytes = Uint8Array.from(packet.bytes)
      stats[record.direction].records++
      stats[record.direction].bytes += bytes.length
      const peek = bytes.subarray(0, 120)
      const end = bytes.length > peek.length ? ' …' : ''
      buffered.push({
        ...record,
        id: nextRow++,
        time: timeFormat.format(record.timestamp),
        packet: { bytes, headerLen: packet.headerLen, tailLen: packet.tailLen },
        hex: formatHex(peek) + end,
        text: displayText(peek) + end,
      })
      bufferedBytes += bytes.length
    }
    trafficStats.value = stats
    let discard = 0
    while (buffered.length - discard > MAX_ROWS || bufferedBytes > MAX_LOG_BYTES) {
      bufferedBytes -= buffered[discard++].packet.bytes.length
    }
    if (discard) buffered.splice(0, discard)
    syncDisplay()
  }

  function resetTrafficStats() {
    trafficStats.value = emptyTrafficStats()
  }

  function clear() {
    buffered.length = 0
    bufferedBytes = 0
    visible.value = []
  }

  function onEvent(event: TransportEvent) {
    switch (event.type) {
      case 'status':
        connected.value = event.connected
        if (event.connected) connectionFailed.value = false
        endpoint.value = event.connected ? event.endpoint : ''
        clients.value = event.clients
        if (!event.connected) {
          running.value = false
          if (retryConnection) {
            reconnecting.value = true
            scheduleReconnect()
          } else {
            activeConnection.value = null
          }
        } else {
          reconnecting.value = false
        }
        break
      case 'messages':
        append(event.rows)
        break
      case 'timer':
        running.value = event.running
        if (event.running) timed.value = true
        break
      case 'fault':
        Message.error(event.message)
    }
  }

  async function refreshPorts() {
    if (refreshing.value) return
    refreshing.value = true
    try {
      ports.value = await transport.listPorts()
    } catch (cause) {
      Message.error(cause)
    } finally {
      refreshing.value = false
    }
  }

  function scheduleReconnect() {
    if (!retryConnection || reconnectTimer !== undefined) return
    reconnecting.value = true
    reconnectTimer = window.setTimeout(() => {
      reconnectTimer = undefined
      if (retryConnection) void attemptConnection(retryConnection, true)
    }, Number(reconnectIntervalMs.value))
  }

  async function attemptConnection(connection: Connection, retry: boolean) {
    if (busy.value) {
      scheduleReconnect()
      return
    }
    if (splitFrames.value && !framing.value.value) {
      if (!retry) Message.error(framing.value.error, 'framing')
      if (retryConnection) scheduleReconnect()
      else connectionFailed.value = true
      return
    }
    busy.value = true
    activeConnection.value = connection
    const current = ++generation
    const rule = splitFrames.value ? framing.value.value : null
    appliedFraming = JSON.stringify(rule)
    try {
      await transport.connect(connection, rule, event => {
        if (generation === current) onEvent(event)
      })
      if (generation === current && connected.value) await applyFraming()
    } catch (cause) {
      if (generation !== current) return
      if (!retry) Message.error(cause, `connection:${current}`)
      if (retryConnection) scheduleReconnect()
      else {
        connectionFailed.value = true
        activeConnection.value = null
      }
    } finally {
      if (generation === current) busy.value = false
    }
  }

  async function connect(connection: Connection) {
    if (busy.value || connected.value || reconnecting.value) return
    connectionFailed.value = false
    if (splitFrames.value && !framing.value.value) {
      connectionFailed.value = true
      Message.error(framing.value.error, 'framing')
      return
    }
    retryConnection = connection.kind === 'tcpClient' && autoReconnect.value ? connection : null
    await attemptConnection(connection, false)
  }

  async function disconnect() {
    if (busy.value && !reconnecting.value) return
    if (!connected.value && !reconnecting.value) return
    retryConnection = null
    if (reconnectTimer !== undefined) window.clearTimeout(reconnectTimer)
    reconnectTimer = undefined
    reconnecting.value = false
    generation++
    busy.value = true
    try {
      await transport.disconnect()
      onEvent({ type: 'status', connected: false, endpoint: '', clients: [] })
    } catch (cause) {
      Message.error(cause)
    } finally {
      busy.value = false
    }
  }

  function notifyFramingError() {
    if ((addFrame.value || splitFrames.value) && framing.value.error) {
      Message.error(framing.value.error, 'framing')
    } else {
      Message.dismiss('framing')
    }
  }

  let appliedFraming = ''
  async function applyFraming() {
    notifyFramingError()
    if (!connected.value || (splitFrames.value && !framing.value.value)) return
    const rule: Framing | null = splitFrames.value ? framing.value.value : null
    const key = JSON.stringify(rule)
    if (appliedFraming === key) return
    const current = generation
    appliedFraming = key
    try {
      await transport.setFraming(rule)
    } catch (cause) {
      if (generation === current && connected.value && appliedFraming === key) {
        appliedFraming = ''
        Message.error(cause)
      }
    }
  }

  async function send() {
    if (!canSend.value || sending.value || !packet.value.value) return
    sending.value = true
    try {
      await transport.send(packet.value.value, target.value)
    } catch (cause) {
      Message.error(cause)
    } finally {
      sending.value = false
    }
  }

  async function toggleTimer() {
    if (timerBusy.value) return
    timerBusy.value = true
    try {
      if (running.value) {
        await transport.stopTimer()
      } else if (canSend.value && validInterval.value && packet.value.value) {
        await transport.startTimer(packet.value.value, target.value, interval.value)
      }
    } catch (cause) {
      Message.error(cause)
    } finally {
      timerBusy.value = false
    }
  }

  async function togglePin() {
    if (pinBusy.value) return
    pinBusy.value = true
    try {
      await transport.setAlwaysOnTop(!pinned.value)
      pinned.value = !pinned.value
    } catch (cause) {
      Message.error(cause)
    } finally {
      pinBusy.value = false
    }
  }

  watch(paused, value => { if (!value) syncDisplay() })
  watch(splitFrames, () => { if (!busy.value) void applyFraming() })
  watch(frameMode, () => { if (!busy.value) void applyFraming() })
  watch(addFrame, notifyFramingError)
  watch(connected, value => { if (!value) appliedFraming = '' })
  onBeforeUnmount(() => {
    retryConnection = null
    if (reconnectTimer !== undefined) window.clearTimeout(reconnectTimer)
    generation++
    if (animation !== undefined) window.cancelAnimationFrame(animation)
    if (connected.value || busy.value) void transport.disconnect().catch(() => {})
  })

  return {
    connected, reconnecting, autoReconnect, reconnectIntervalMs, activeConnection, connectionIndicator, endpoint, clients, ports, busy, refreshing, sending, timerBusy, running,
    pinned, pinBusy, target, input, sendMode, timed, intervalMs, addFrame, splitFrames, frameMode, headerInput,
    tailInput, paused, visible, trafficStats, framing, packet, validInterval, canSend, clear, resetTrafficStats, refreshPorts, connect,
    disconnect, applyFraming, setFrameMode, send, toggleTimer, togglePin,
  }
}
