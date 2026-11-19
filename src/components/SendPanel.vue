<script setup lang="ts">
import { computed, nextTick, shallowRef } from 'vue'
import { ArrowUpFromLine } from '@lucide/vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import { Checkbox } from '@/components/ui/checkbox'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Field } from '@/components/ui/field'
import { Label } from '@/components/ui/label'
import { NumberField, NumberFieldContent, NumberFieldDecrement, NumberFieldIncrement, NumberFieldInput } from '@/components/ui/number-field'
import { Textarea } from '@/components/ui/textarea'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { displayText, formatHex, formatHexLines, parseHex } from '@/lib/bytes'
import type { Packet } from '@/lib/bytes'
import { Message } from '@/lib/message'

const props = defineProps<{
  packet: Packet | null
  frameMode: 'hex' | 'text'
  inputError: string
  canSend: boolean
  sending: boolean
  running: boolean
  timerBusy: boolean
  validInterval: boolean
}>()
const emit = defineEmits<{ send: []; toggleTimer: [] }>()

const input = defineModel<string>('input', { required: true })
const mode = defineModel<'text' | 'hex'>('mode', { required: true })
const timed = defineModel<boolean>('timed', { required: true })
const interval = defineModel<number | undefined>('interval', { required: true })
const previewOpen = shallowRef(false)
const numberFormat = { useGrouping: false, maximumFractionDigits: 10 }

function reportInputError() {
  if (props.inputError) Message.error(props.inputError, 'send-input')
  else Message.dismiss('send-input')
}

async function selectMode(value: unknown) {
  if ((value !== 'text' && value !== 'hex') || mode.value === value) return
  let converted: string
  if (value === 'hex') {
    for (const character of input.value) {
      const code = character.codePointAt(0)!
      if (code >= 0xd800 && code <= 0xdfff) {
        Message.error('TXT包含无效的 Unicode 字符')
        return
      }
    }
    converted = formatHex(new TextEncoder().encode(input.value))
  } else {
    let bytes: number[]
    try {
      bytes = parseHex(input.value)
    } catch (error) {
      Message.error(error)
      return
    }
    try {
      converted = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(Uint8Array.from(bytes))
    } catch {
      Message.error('HEX 包含无效的 UTF-8 字节，无法转换为TXT')
      return
    }
    if (converted.includes('\r')) {
      Message.error('HEX 包含回车字符，TXT输入框无法保留原始字节')
      return
    }
  }
  input.value = converted
  mode.value = value
  await nextTick()
  reportInputError()
}

function framePreview(bytes: number[]) {
  return props.frameMode === 'text' ? displayText(Uint8Array.from(bytes)) : formatHex(bytes)
}

const length = computed(() => props.packet?.bytes.length ?? 0)
const long = computed(() => length.value > 20)
const summary = computed(() => {
  const packet = props.packet
  if (!packet) return []
  const pieces: { text: string; boundary: boolean }[] = []
  if (mode.value === 'text') {
    const payload = Uint8Array.from(packet.bytes.slice(packet.headerLen, packet.bytes.length - packet.tailLen))
    const characters = Array.from(displayText(payload))
    const text = long.value && characters.length > 36
      ? characters.slice(0, 24).join('') + ' … ' + characters.slice(-12).join('')
      : characters.join('')
    if (packet.headerLen) pieces.push({ text: framePreview(packet.bytes.slice(0, packet.headerLen)), boundary: true })
    if (text) pieces.push({ text, boundary: false })
    if (packet.tailLen) pieces.push({ text: framePreview(packet.bytes.slice(-packet.tailLen)), boundary: true })
    return pieces
  }
  const end = packet.bytes.length
  const tailStart = end - packet.tailLen
  const ranges = long.value ? [[0, 8], [end - 6, end]] : [[0, end]]
  for (const [start, stop] of ranges) {
    if (start) pieces.push({ text: '…', boundary: false })
    let cursor = start
    while (cursor < stop) {
      const boundary = cursor < packet.headerLen || cursor >= tailStart
      const next = Math.min(stop, cursor < packet.headerLen ? packet.headerLen : cursor < tailStart ? tailStart : end)
      pieces.push({ text: formatHex(packet.bytes.slice(cursor, next)), boundary })
      cursor = next
    }
  }
  return pieces
})
const fullPreview = computed(() => {
  const packet = props.packet
  if (!previewOpen.value || !packet) return ''
  if (mode.value === 'hex') return formatHexLines(packet.bytes)
  const payload = new TextDecoder().decode(Uint8Array.from(packet.bytes.slice(packet.headerLen, packet.bytes.length - packet.tailLen)))
  const parts = []
  if (packet.headerLen) parts.push('帧头（' + (props.frameMode === 'hex' ? 'HEX' : 'TXT') + '） ' + framePreview(packet.bytes.slice(0, packet.headerLen)))
  parts.push(packet.headerLen || packet.tailLen ? '载荷（TXT）\n' + payload : payload)
  if (packet.tailLen) parts.push('帧尾（' + (props.frameMode === 'hex' ? 'HEX' : 'TXT') + '） ' + framePreview(packet.bytes.slice(-packet.tailLen)))
  return parts.join('\n')
})

async function copy() {
  try {
    await navigator.clipboard.writeText(fullPreview.value)
    Message.success(mode.value === 'hex' ? '已复制 HEX' : '已复制TXT')
  } catch {
    Message.error('复制失败，请选中内容后手动复制')
  }
}
</script>

<template>
  <Card class="panel send-panel" role="region" aria-labelledby="send-title">
    <header class="workspace-header">
      <h2 id="send-title" class="panel-heading"><ArrowUpFromLine />发送区</h2>
      <ToggleGroup class="send-modes" type="single" size="sm" :model-value="mode" aria-label="发送输入格式"
        @update:model-value="selectMode">
        <ToggleGroupItem value="text">TXT</ToggleGroupItem>
        <ToggleGroupItem value="hex">HEX</ToggleGroupItem>
      </ToggleGroup>
    </header>
    <CardContent class="send-body">
      <Field class="gap-1" :data-invalid="!!inputError">
        <Textarea v-model="input" class="mono send-input" aria-label="发送内容"
          :aria-invalid="!!inputError" :placeholder="mode === 'hex' ? '输入 HEX，例如 01 03' : '输入 UTF-8 文本'"
          spellcheck="false" autocapitalize="off" autocomplete="off" @blur="reportInputError" />
      </Field>
      <div class="wire-preview" aria-label="实际发送预览">
        <span class="wire-label">实际发送</span>
        <div class="wire-sample mono">
          <template v-if="length">
            <span v-for="(part, index) in summary" :key="index" :class="{ boundary: part.boundary }">{{ part.text }}</span>
          </template>
          <span v-else class="muted">—</span>
        </div>
        <span class="wire-length mono">{{ length }} 字节</span>
        <Button v-if="long" variant="outline" size="sm" class="view-packet" @click="previewOpen = true">查看全部</Button>
      </div>

      <div class="send-footer">
        <div class="timer-controls">
          <div class="check-row">
            <Checkbox id="timed" v-model="timed" :disabled="running || timerBusy" />
            <Label for="timed">定时发送</Label>
          </div>
          <template v-if="timed || running">
            <NumberField id="interval-ms" v-model="interval" :min="1" :max="2147483647" :format-options="numberFormat"
              class="w-28" :disabled="running || timerBusy" allow-invalid disable-wheel-change>
              <NumberFieldContent>
                <NumberFieldDecrement class="p-1.5" aria-label="减少定时间隔" />
                <NumberFieldInput class="mono" aria-label="定时间隔" :aria-invalid="!validInterval" />
                <NumberFieldIncrement class="p-1.5" aria-label="增加定时间隔" />
              </NumberFieldContent>
            </NumberField>
            <span class="muted">ms</span>
          </template>
        </div>
        <div class="send-actions">
          <Button :variant="timed || running ? 'outline' : 'default'" :disabled="!canSend || sending" @click="emit('send')">
            {{ sending ? '发送中…' : '发送一次' }}
          </Button>
          <Button v-if="timed || running" :disabled="timerBusy || (!running && (!canSend || !validInterval))"
            @click="emit('toggleTimer')">
            {{ running ? '停止定时发送' : '开始定时发送' }}
          </Button>
        </div>
      </div>
    </CardContent>

    <Dialog v-model:open="previewOpen">
      <DialogContent class="packet-dialog sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>实际发送字节</DialogTitle>
          <DialogDescription>{{ length }} 字节 · {{ mode === 'hex' ? 'HEX' : 'TXT' }}</DialogDescription>
        </DialogHeader>
        <pre class="full-packet mono" tabindex="0" aria-label="完整发送字节">{{ fullPreview }}</pre>
        <DialogFooter>
          <Button @click="copy">{{ mode === 'hex' ? '复制 HEX' : '复制TXT' }}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </Card>
</template>
