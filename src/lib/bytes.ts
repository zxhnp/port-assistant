export interface Packet {
  bytes: number[]
  headerLen: number
  tailLen: number
}

export interface Framing {
  header: number[]
  tail: number[]
}

export const MAX_PACKET_BYTES = 1024 * 1024
const hexByte = Array.from({ length: 256 }, (_, byte) =>
  byte.toString(16).padStart(2, '0').toUpperCase(),
)
const encoder = new TextEncoder()
const decoder = new TextDecoder()

export function parseHex(input: string, limit = MAX_PACKET_BYTES): number[] {
  const digits = input.replace(/\s/g, '')
  if (digits.length % 2 || /[^0-9a-f]/i.test(digits)) {
    throw new Error('HEX 请输入成对的十六进制字符，可用空格或换行分隔')
  }
  if (digits.length > limit * 2) throw new Error(`最多支持 ${limit} 字节`)
  const bytes = new Array<number>(digits.length / 2)
  for (let index = 0; index < bytes.length; index++) {
    bytes[index] = Number.parseInt(digits.slice(index * 2, index * 2 + 2), 16)
  }
  return bytes
}

function parseFrameText(input: string): number[] {
  const value = input.replace(/\\(u\{[\da-fA-F]{1,6}\}|u[\da-fA-F]{3,4}|[nrt\\]|.|$)/g, (_match, escape: string) => {
    if (escape === 'n') return '\n'
    if (escape === 'r') return '\r'
    if (escape === 't') return '\t'
    if (escape === '\\') return '\\'
    if (/^u(?:[\da-fA-F]{3,4}|\{[\da-fA-F]{1,6}\})$/.test(escape)) {
      const digits = escape.startsWith('u{') ? escape.slice(2, -1) : escape.slice(1)
      const code = Number.parseInt(digits, 16)
      if (code > 0x10ffff || (escape.startsWith('u{') && code >= 0xd800 && code <= 0xdfff)) {
        throw new Error('Unicode 字符无效')
      }
      return String.fromCodePoint(code)
    }
    throw new Error('帧头帧尾包含无效转义字符')
  })
  for (const char of value) {
    const code = char.codePointAt(0)!
    if (code >= 0xd800 && code <= 0xdfff) throw new Error('Unicode 字符无效')
  }
  const bytes = Array.from(encoder.encode(value))
  if (bytes.length > 1024) throw new Error('帧头帧尾各最多支持 1024 字节')
  return bytes
}

export function parseFraming(header: string, tail: string, mode: 'hex' | 'text' = 'hex'): Framing {
  const parse = mode === 'hex' ? (value: string) => parseHex(value, 1024) : parseFrameText
  const framing = { header: parse(header), tail: parse(tail) }
  if (!framing.header.length || !framing.tail.length) {
    throw new Error('请填写帧头和帧尾')
  }
  return framing
}

export function makePacket(input: string, mode: 'text' | 'hex', framing: Framing | null): Packet {
  const payload = mode === 'hex' ? parseHex(input) : Array.from(encoder.encode(input))
  const headerLen = framing?.header.length ?? 0
  const tailLen = framing?.tail.length ?? 0
  if (payload.length + headerLen + tailLen > MAX_PACKET_BYTES) {
    throw new Error('实际发送内容最多支持 1 MiB')
  }
  const bytes = framing ? framing.header.concat(payload, framing.tail) : payload
  return { bytes, headerLen, tailLen }
}

export function formatHex(bytes: ArrayLike<number>): string {
  return Array.from(bytes, byte => hexByte[byte]).join(' ')
}

export function formatHexLines(bytes: ArrayLike<number>): string {
  const lines: string[] = []
  for (let offset = 0; offset < bytes.length; offset += 16) {
    const line: string[] = []
    for (let index = offset; index < Math.min(offset + 16, bytes.length); index++) {
      line.push(hexByte[bytes[index]])
    }
    lines.push(line.join(' '))
  }
  return lines.join('\n')
}

export function displayText(bytes: Uint8Array): string {
  return decoder.decode(bytes).replace(/[\x00-\x1F\x7F]/g, char => {
    if (char === '\r') return '\\r'
    if (char === '\n') return '\\n'
    if (char === '\t') return '\\t'
    return `\\x${hexByte[char.charCodeAt(0)]}`
  })
}
