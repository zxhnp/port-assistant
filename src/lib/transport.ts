import { Channel, invoke, isTauri } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import type { Framing, Packet } from './bytes'

export type Protocol = 'serial' | 'udp' | 'tcp'
export interface Address { host: string; port: number }
export interface SerialConnection {
  kind: 'serial'
  port: string
  baudRate: number
  dataBits: number
  stopBits: number
  parity: 'none' | 'odd' | 'even' | 'mark' | 'space'
  flowControl: 'none' | 'hardware' | 'software'
}
export type Connection = SerialConnection
  | { kind: 'udp'; local: Address; mode: 'unicast' | 'broadcast' | 'multicast'; group?: string; multicastTtl?: number }
  | { kind: 'tcpServer'; local: Address }
  | { kind: 'tcpClient'; remote: Address }
export type Target = { kind: 'peer'; id: number } | ({ kind: 'udp' } & Address)
export interface Peer { id: number; address: string }
export interface WireRecord {
  direction: 'rx' | 'tx'
  timestamp: number
  peer: string | null
  packet: Packet
}

export type TransportEvent =
  | { type: 'status'; connected: boolean; endpoint: string; clients: Peer[] }
  | { type: 'messages'; rows: WireRecord[] }
  | { type: 'timer'; running: boolean }
  | { type: 'fault'; message: string }

interface Transport {
  simulated: boolean
  listPorts(): Promise<string[]>
  connect(connection: Connection, framing: Framing | null, events: (event: TransportEvent) => void): Promise<void>
  disconnect(): Promise<void>
  setFraming(framing: Framing | null): Promise<void>
  send(packet: Packet, target: Target | null): Promise<void>
  startTimer(packet: Packet, target: Target | null, intervalMs: number): Promise<void>
  stopTimer(): Promise<void>
  setAlwaysOnTop(value: boolean): Promise<void>
}

function createDesktopTransport(): Transport {
  return {
    simulated: false,
    listPorts: () => invoke<string[]>('list_ports'),
    connect(connection, framing, events) {
      const channel = new Channel<TransportEvent>(events)
      return invoke('connect', { connection, framing, events: channel })
    },
    disconnect: () => invoke('disconnect'),
    setFraming: framing => invoke('set_framing', { framing }),
    send: (packet, target) => invoke('send_packet', { packet, target }),
    startTimer: (packet, target, intervalMs) => invoke('start_timer', { packet, target, intervalMs }),
    stopTimer: () => invoke('stop_timer'),
    setAlwaysOnTop: value => getCurrentWindow().setAlwaysOnTop(value),
  }
}

const mockPeers: Peer[] = [
  { id: 1, address: '127.0.0.1:50421' },
  { id: 2, address: '127.0.0.1:50422' },
]

function findSequence(bytes: Uint8Array, delimiter: number[], start: number): number {
  let index = bytes.indexOf(delimiter[0], start)
  while (index >= 0 && index + delimiter.length <= bytes.length) {
    if (delimiter.every((byte, offset) => bytes[index + offset] === byte)) return index
    index = bytes.indexOf(delimiter[0], index + 1)
  }
  return -1
}

function mockReceive(packet: Packet, framing: Framing | null): Packet[] {
  if (!framing) return [{ bytes: packet.bytes, headerLen: 0, tailLen: 0 }]
  const { header, tail } = framing
  const bytes = Uint8Array.from(packet.bytes)
  const packets: Packet[] = []
  let current = 0
  while (current < bytes.length) {
    const start = findSequence(bytes, header, current)
    if (start < 0) break
    const end = findSequence(bytes, tail, start + header.length)
    if (end < 0) break
    current = end + tail.length
    packets.push({
      bytes: Array.from(bytes.subarray(start, current)),
      headerLen: header.length,
      tailLen: tail.length,
    })
  }
  return packets
}

async function checkLocalConnection(connection: Connection, signal: AbortSignal) {
  if (!import.meta.env.DEV) throw new Error('请从本地开发服务或桌面版打开连接')
  let response: Response
  try {
    response = await fetch('/__dev/check-connection', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(connection),
      signal: AbortSignal.any([signal, AbortSignal.timeout(7000)]),
    })
  } catch {
    throw new Error('本机连接检查未完成，请重试')
  }
  const result = await response.json().catch(() => ({})) as { endpoint?: string; error?: string }
  if (!response.ok || !result.endpoint) throw new Error(result.error ?? '本机连接检查失败')
  return result.endpoint
}

function createMockTransport(): Transport {
  let emit: ((event: TransportEvent) => void) | null = null
  let framing: Framing | null = null
  let connection: Connection | null = null
  let opening: AbortController | null = null
  let endpoint = ''
  let timer: number | undefined

  function peerFor(packet: Packet, target: Target | null): string {
    if (!connection) throw new Error('当前没有活动连接')
    if (connection.kind === 'udp') {
      if (target?.kind !== 'udp' || !target.host || !target.port) {
        throw new Error('请填写 UDP 目的地址和端口')
      }
      if (packet.bytes.length > 65_507) throw new Error('UDP 单条消息最多支持 65507 字节')
      return `${target.host}:${target.port}`
    }
    if (connection.kind === 'tcpServer') {
      const peer = target?.kind === 'peer' && mockPeers.find(peer => peer.id === target.id)
      if (!peer) throw new Error('请选择有效的发送目标')
      return peer.address
    }
    return endpoint
  }

  function push(packet: Packet, peer: string) {
    if (!emit) return
    const timestamp = Date.now()
    const received = mockReceive(packet, framing)
    emit({
      type: 'messages',
      rows: [
        { direction: 'tx', timestamp, peer, packet },
        ...received.map(packet => ({ direction: 'rx' as const, timestamp, peer, packet })),
      ],
    })
  }

  function stop() {
    if (timer === undefined) return
    window.clearInterval(timer)
    timer = undefined
    emit?.({ type: 'timer', running: false })
  }

  return {
    simulated: true,
    async listPorts() {
      if (!import.meta.env.DEV) return []
      const response = await fetch('/__dev/serial-ports', { cache: 'no-store' })
      if (!response.ok) throw new Error('读取本机串口列表失败，请重试')
      return await response.json() as string[]
    },
    async connect(options, receiveFraming, events) {
      if (connection || opening) throw new Error('请先关闭当前连接')
      const request = new AbortController()
      opening = request
      try {
        const checkedEndpoint = await checkLocalConnection(options, request.signal)
        request.signal.throwIfAborted()
        endpoint = checkedEndpoint
      } finally {
        if (opening === request) opening = null
      }
      connection = options
      framing = receiveFraming
      emit = events
      emit({
        type: 'status',
        connected: true,
        endpoint,
        clients: options.kind === 'tcpServer' ? mockPeers : [],
      })
    },
    async disconnect() {
      opening?.abort()
      opening = null
      stop()
      emit?.({ type: 'status', connected: false, endpoint: '', clients: [] })
      connection = null
      emit = null
    },
    async setFraming(value) { framing = value },
    async send(packet, target) { push(packet, peerFor(packet, target)) },
    async startTimer(packet, target, intervalMs) {
      if (timer !== undefined) throw new Error('定时发送正在运行')
      const peer = peerFor(packet, target)
      timer = window.setInterval(() => push(packet, peer), intervalMs)
      emit?.({ type: 'timer', running: true })
      push(packet, peer)
    },
    async stopTimer() { stop() },
    async setAlwaysOnTop() { /* 网页仅模拟置顶按钮状态。 */ },
  }
}

export const transport = isTauri() ? createDesktopTransport() : createMockTransport()
