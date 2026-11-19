<script setup lang="ts">
import { computed, nextTick, shallowRef, useTemplateRef, watch } from 'vue'
import { ArrowDownToLine, FileText, X } from '@lucide/vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Checkbox } from '@/components/ui/checkbox'
import { Label } from '@/components/ui/label'
import { RadioGroup, RadioGroupItem } from '@/components/ui/radio-group'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { Toggle } from '@/components/ui/toggle'
import { Message } from '@/lib/message'
import { formatHex, formatHexLines } from '@/lib/bytes'
import type { LogRow } from '@/composables/useWorkbench'

const props = defineProps<{ rows: LogRow[]; connected: boolean }>()
const emit = defineEmits<{ clear: [] }>()
const paused = defineModel<boolean>('paused', { required: true })
const showTime = defineModel<boolean>('showTime', { required: true })
const autoScroll = defineModel<boolean>('autoScroll', { required: true })
const mode = shallowRef<'text' | 'hex'>('text')
const direction = shallowRef<'all' | 'rx' | 'tx'>('all')
const selected = shallowRef<number | null>(null)
const detailMode = shallowRef<'text' | 'hex'>('text')
const formatJson = shallowRef(false)
const logList = useTemplateRef<HTMLElement>('logList')

function parseJsonText(text: string): unknown {
  let inString = false
  let escaped = false
  let normalized = ''
  for (const char of text) {
    if (inString && !escaped && (char === '\r' || char === '\n' || char === '\t')) {
      normalized += char === '\r' ? '\\r' : char === '\n' ? '\\n' : '\\t'
      continue
    }
    normalized += char
    if (escaped) {
      escaped = false
    } else if (inString && char === '\\') {
      escaped = true
    } else if (char === '"') {
      inString = !inString
    }
  }
  return JSON.parse(normalized)
}

const visibleRows = computed(() => direction.value === 'all'
  ? props.rows
  : props.rows.filter(row => row.direction === direction.value))
const active = computed(() => visibleRows.value.find(row => row.id === selected.value))
const details = computed(() => {
  const packet = active.value?.packet
  if (!packet) return null
  const { bytes, headerLen, tailLen } = packet
  const payload = bytes.subarray(headerLen, bytes.length - tailLen)
  let text = new TextDecoder().decode(payload)
  if (detailMode.value === 'text' && formatJson.value) {
    try {
      text = JSON.stringify(parseJsonText(text), null, 2)
    } catch {
      // 新选择的记录可能不是 JSON，保留原始TXT。
    }
  }
  return {
    header: formatHex(bytes.subarray(0, headerLen)),
    payload: detailMode.value === 'hex' ? formatHexLines(payload) : text,
    tail: formatHex(bytes.subarray(bytes.length - tailLen)),
    length: bytes.length,
  }
})

async function scrollToBottom() {
  if (!autoScroll.value || paused.value) return
  await nextTick()
  if (logList.value) logList.value.scrollTop = logList.value.scrollHeight
}

function clear() {
  selected.value = null
  emit('clear')
}

function selectRow(id: number) {
  if (selected.value !== id) formatJson.value = false
  selected.value = id
}

function toggleJson(value: boolean) {
  if (!value) {
    formatJson.value = false
    return
  }
  const packet = active.value?.packet
  if (!packet) return
  const payload = packet.bytes.subarray(packet.headerLen, packet.bytes.length - packet.tailLen)
  try {
    parseJsonText(new TextDecoder().decode(payload))
    formatJson.value = true
  } catch {
    Message.error('当前内容不是有效的 JSON')
  }
}

watch([visibleRows, autoScroll, paused], () => void scrollToBottom())
watch(direction, () => { selected.value = null })
</script>

<template>
  <Card class="panel receive-panel" role="region" aria-labelledby="receive-title">
    <header class="workspace-header">
      <h2 id="receive-title" class="panel-heading"><ArrowDownToLine />通信记录</h2>
      <div class="receive-tools">
        <RadioGroup v-model="direction" class="direction-filter" aria-label="按收发方向筛选">
          <div class="check-row"><RadioGroupItem id="direction-all" value="all" /><Label for="direction-all">全部</Label></div>
          <div class="check-row"><RadioGroupItem id="direction-rx" value="rx" /><Label for="direction-rx">RX 接收</Label></div>
          <div class="check-row"><RadioGroupItem id="direction-tx" value="tx" /><Label for="direction-tx">TX 发送</Label></div>
        </RadioGroup>
        <div class="check-row">
          <Checkbox id="show-time" v-model="showTime" />
          <Label for="show-time">时间戳</Label>
        </div>
        <div class="check-row">
          <Checkbox id="auto-scroll" v-model="autoScroll" />
          <Label for="auto-scroll">自动滚动</Label>
        </div>
        <RadioGroup v-model="mode" class="receive-modes" aria-label="通信记录显示格式">
          <div class="check-row">
            <RadioGroupItem id="receive-text" value="text" />
            <Label for="receive-text">TXT</Label>
          </div>
          <div class="check-row">
            <RadioGroupItem id="receive-hex" value="hex" />
            <Label for="receive-hex">HEX</Label>
          </div>
        </RadioGroup>
        <div class="log-actions">
          <Button variant="outline" size="sm" :aria-pressed="paused" @click="paused = !paused">
            {{ paused ? '继续' : '暂停' }}
          </Button>
          <Button variant="outline" size="sm" title="清空全部收发记录" @click="clear">清空</Button>
        </div>
      </div>
    </header>

    <div class="receive-body">
      <div ref="logList" class="log-list" aria-label="收发记录">
        <div v-if="!rows.length" class="empty-log">
          {{ connected ? '等待数据…' : '打开连接后，收发记录将在这里显示' }}
        </div>
        <div v-else-if="!visibleRows.length" class="empty-log">暂无符合筛选条件的记录</div>
        <Button v-for="row in visibleRows" :key="row.id" v-memo="[row, selected === row.id, showTime, mode]"
          type="button" variant="ghost" class="log-row mono" :aria-pressed="selected === row.id" @click="selectRow(row.id)">
          <time v-if="showTime" class="log-time" :datetime="new Date(row.timestamp).toISOString()">[{{ row.time }}]</time>
          <Badge variant="secondary" class="direction-mark" :class="row.direction">{{ row.direction.toUpperCase() }}</Badge>
          <span class="log-payload">{{ mode === 'hex' ? row.hex : row.text }}</span>
        </Button>
      </div>

      <Card v-if="active && details" class="panel frame-detail" role="complementary" aria-labelledby="detail-title">
        <header>
          <h3 id="detail-title"><FileText />帧详情</h3>
          <Tooltip>
            <TooltipTrigger as-child>
              <Button variant="ghost" size="icon-xs" aria-label="关闭帧详情" @click="selected = null"><X /></Button>
            </TooltipTrigger>
            <TooltipContent>关闭帧详情</TooltipContent>
          </Tooltip>
        </header>
        <div class="detail-meta" aria-label="帧信息">
          <span>长度 <strong>{{ details.length }} 字节</strong></span>
          <span v-if="active.peer">对端 <strong>{{ active.peer }}</strong></span>
        </div>
        <div class="detail-toolbar">
        <RadioGroup v-model="detailMode" class="detail-modes" aria-label="详情显示格式">
          <div class="check-row"><RadioGroupItem id="detail-text" value="text" /><Label for="detail-text">TXT</Label></div>
          <div class="check-row"><RadioGroupItem id="detail-hex" value="hex" /><Label for="detail-hex">HEX</Label></div>
        </RadioGroup>
        <Toggle v-if="detailMode === 'text'" class="detail-json" variant="outline" size="sm"
          :model-value="formatJson" @update:model-value="toggleJson">JSON 格式化</Toggle>
        </div>
        <dl>
          <div v-if="details.header"><dt>帧头</dt><dd class="mono boundary">{{ details.header }}</dd></div>
          <div><dt>{{ details.header || details.tail ? '载荷' : '内容' }}</dt>
            <dd><pre class="mono detail-payload">{{ details.payload || '—' }}</pre></dd>
          </div>
          <div v-if="details.tail"><dt>帧尾</dt><dd class="mono boundary">{{ details.tail }}</dd></div>
        </dl>
      </Card>
    </div>
  </Card>
</template>
