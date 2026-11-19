<script setup lang="ts">
import { computed, reactive, shallowRef, watch } from 'vue'
import { Activity, ArrowLeftRight, Cable, Check, ChevronDown, Globe2, Hash, Layers3, MapPin, Network, Radio, RefreshCw, Settings, Square, Target as TargetIcon } from '@lucide/vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Card, CardContent } from '@/components/ui/card'
import {
  Combobox, ComboboxAnchor, ComboboxEmpty, ComboboxGroup, ComboboxInput, ComboboxItem,
  ComboboxItemIndicator, ComboboxList, ComboboxTrigger,
} from '@/components/ui/combobox'
import { Field, FieldGroup, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { NumberField, NumberFieldInput } from '@/components/ui/number-field'
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import type { ConnectionIndicator } from '@/composables/useWorkbench'
import type { Connection, Peer, Protocol, SerialConnection, Target } from '@/lib/transport'

const props = defineProps<{
  ports: string[]
  clients: Peer[]
  connected: boolean
  reconnecting: boolean
  indicator: ConnectionIndicator
  busy: boolean
  refreshing: boolean
  endpoint: string
  framingInvalid: boolean
}>()

const emit = defineEmits<{
  connect: [connection: Connection]
  disconnect: []
  refresh: []
  applyFraming: []
}>()

const protocol = defineModel<Protocol>('protocol', { required: true })
const target = defineModel<Target | null>('target', { required: true })
const autoReconnect = defineModel<boolean>('autoReconnect', { required: true })
const reconnectInterval = defineModel<number | undefined>('reconnectInterval', { required: true })
const addFrame = defineModel<boolean>('addFrame', { required: true })
const splitFrames = defineModel<boolean>('splitFrames', { required: true })
const frameMode = defineModel<'hex' | 'text'>('frameMode', { required: true })
const header = defineModel<string>('header', { required: true })
const tail = defineModel<string>('tail', { required: true })

const protocols = [
  { id: 'serial', title: '串口', icon: Cable },
  { id: 'udp', title: 'UDP', icon: Network },
  { id: 'tcp', title: 'TCP', icon: Layers3 },
] as const
const serial = reactive<SerialConnection>({
  kind: 'serial', port: '', baudRate: 9600, dataBits: 8, stopBits: 1, parity: 'none', flowControl: 'none',
})
const udp = reactive({ mode: 'unicast' as 'unicast' | 'broadcast' | 'multicast', host: '0.0.0.0', port: 9000 as number | undefined, remoteHost: '127.0.0.1', remotePort: 9001 as number | undefined, group: '239.255.0.1', multicastTtl: 255 as number | undefined })
const tcp = reactive({ mode: 'tcpClient' as 'tcpClient' | 'tcpServer', host: '127.0.0.1', port: 9000 as number | undefined })
const serverAddress = reactive({ host: '0.0.0.0', port: 9000 as number | undefined })
const peer = shallowRef('')
const baudQuery = shallowRef('')
const baudPresets = [1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600]
const numberFormat = { useGrouping: false, maximumFractionDigits: 10 }

const activeAddress = computed(() => tcp.mode === 'tcpClient' ? tcp : serverAddress)
const locked = computed(() => props.connected || props.busy || props.reconnecting)
const tcpMinPort = computed(() => tcp.mode === 'tcpClient' ? 1 : 0)
const validPort = (value: number | undefined, min: number) =>
  value !== undefined && Number.isInteger(value) && value >= min && value <= 65_535
const validBaud = (value: number) => Number.isInteger(value) && value >= 1 && value <= 4_294_967_295
const visibleBauds = computed(() => baudPresets.filter(rate => String(rate).includes(baudQuery.value.trim())))
const customBaud = computed(() => {
  const input = baudQuery.value.trim()
  const rate = Number(input)
  return /^\d+$/.test(input) && validBaud(rate) && !baudPresets.includes(rate) ? rate : null
})

function selectProtocol(value: unknown) {
  if (!locked.value && (value === 'serial' || value === 'udp' || value === 'tcp')) protocol.value = value
}

function selectBaud(value: unknown) {
  const rate = Number(value)
  if (validBaud(rate)) serial.baudRate = rate
}

function resetBaudQuery(open: boolean) {
  if (!open) baudQuery.value = ''
}
const canOpen = computed(() => {
  if (protocol.value === 'serial') {
    return !!serial.port && validBaud(serial.baudRate)
  }
  if (protocol.value === 'udp') return !!udp.host.trim() && validPort(udp.port, 0)
    && (udp.mode !== 'multicast' || (!!udp.group.trim() && validPort(udp.multicastTtl, 0) && Number(udp.multicastTtl) <= 255))
  return !!activeAddress.value.host.trim() && validPort(activeAddress.value.port, tcpMinPort.value)
    && (tcp.mode !== 'tcpClient' || !autoReconnect.value || (reconnectInterval.value !== undefined
      && Number.isInteger(reconnectInterval.value) && reconnectInterval.value >= 100 && reconnectInterval.value <= 2_147_483_647))
})

watch(() => udp.mode, mode => {
  if (mode === 'broadcast' && udp.remoteHost === '127.0.0.1') udp.remoteHost = '255.255.255.255'
  else if (mode === 'unicast' && udp.remoteHost === '255.255.255.255') udp.remoteHost = '127.0.0.1'
})

watch(() => props.ports, options => {
  if (!locked.value && !options.includes(serial.port)) serial.port = options[0] ?? ''
}, { immediate: true })

watch(() => props.clients, options => {
  if (!options.some(client => String(client.id) === peer.value)) peer.value = ''
})

watch([protocol, () => udp.mode, () => udp.remoteHost, () => udp.remotePort, () => udp.group, () => tcp.mode, peer], () => {
  if (protocol.value === 'udp') {
    target.value = { kind: 'udp', host: (udp.mode === 'multicast' ? udp.group : udp.remoteHost).trim(), port: Number(udp.remotePort) }
  } else if (protocol.value === 'tcp' && tcp.mode === 'tcpServer' && peer.value) {
    target.value = { kind: 'peer', id: Number(peer.value) }
  } else {
    target.value = null
  }
}, { immediate: true })

function toggle() {
  if (props.busy && !props.reconnecting) return
  if (props.connected || props.reconnecting) {
    emit('disconnect')
    return
  }
  if (!canOpen.value) return
  if (protocol.value === 'serial') {
    emit('connect', { ...serial, baudRate: Number(serial.baudRate) })
  } else if (protocol.value === 'udp') {
    emit('connect', { kind: 'udp', local: { host: udp.host.trim(), port: Number(udp.port) }, mode: udp.mode,
      ...(udp.mode === 'multicast' ? { group: udp.group.trim(), multicastTtl: Number(udp.multicastTtl) } : {}) })
  } else {
    const address = { host: activeAddress.value.host.trim(), port: Number(activeAddress.value.port) }
    emit('connect', tcp.mode === 'tcpClient'
      ? { kind: 'tcpClient', remote: address }
      : { kind: 'tcpServer', local: address })
  }
}

const actionLabel = computed(() => {
  if (props.reconnecting) return '停止重连'
  if (props.busy) return '处理中…'
  if (props.connected) return '断开'
  if (protocol.value === 'serial') return '打开串口'
  if (protocol.value === 'tcp' && tcp.mode === 'tcpClient') return '连接'
  return '开始监听'
})

defineExpose({ toggle, canOpen })
</script>

<template>
  <aside class="operations" aria-label="连接与帧设置">
    <ToggleGroup class="protocol-switch" type="single" :model-value="protocol" aria-label="通信协议"
      @update:model-value="selectProtocol">
      <ToggleGroupItem v-for="item in protocols" :key="item.id" :value="item.id"
        :disabled="locked && protocol !== item.id">
        <component :is="item.icon" aria-hidden="true" />
        {{ item.title }}
      </ToggleGroupItem>
    </ToggleGroup>

    <Card class="panel connection-panel" role="region" aria-label="连接设置">
      <form @submit.prevent="toggle">
        <FieldGroup class="connection-form">
          <template v-if="protocol === 'serial'">
            <Field orientation="horizontal" class="field-row">
              <FieldLabel for="serial-port"><Cable />端口</FieldLabel>
              <div class="port-picker">
                <Select v-model="serial.port" :disabled="locked || !ports.length">
                  <SelectTrigger id="serial-port" class="w-full min-w-0">
                    <SelectValue :placeholder="refreshing ? '查找中…' : '未发现串口'" />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectGroup>
                      <SelectItem v-if="serial.port && !ports.includes(serial.port)" :value="serial.port">{{ serial.port }}</SelectItem>
                      <SelectItem v-for="port in ports" :key="port" :value="port">{{ port }}</SelectItem>
                    </SelectGroup>
                  </SelectContent>
                </Select>
                <Tooltip>
                  <TooltipTrigger as-child>
                    <Button type="button" size="icon-sm" variant="ghost" aria-label="刷新串口"
                      :disabled="locked || refreshing" @click="emit('refresh')">
                      <RefreshCw :class="{ 'animate-spin': refreshing }" />
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent>刷新串口</TooltipContent>
                </Tooltip>
              </div>
            </Field>
            <Field orientation="horizontal" class="field-row">
              <FieldLabel id="baud-rate-label" for="baud-rate"><Activity />波特率</FieldLabel>
              <Combobox class="min-w-0" :model-value="String(serial.baudRate)" :disabled="locked" ignore-filter
                @update:model-value="selectBaud" @update:open="resetBaudQuery">
                <ComboboxAnchor as-child>
                  <ComboboxTrigger as-child>
                    <Button id="baud-rate" aria-labelledby="baud-rate-label" type="button" variant="outline" class="w-full min-w-0 justify-between font-normal">
                      <span class="min-w-0 truncate">{{ serial.baudRate }}</span>
                      <ChevronDown class="text-muted-foreground opacity-50" />
                    </Button>
                  </ComboboxTrigger>
                </ComboboxAnchor>
                <ComboboxList align="start" class="min-w-[13rem]">
                  <ComboboxInput v-model="baudQuery" aria-label="搜索或输入波特率" placeholder="搜索或输入波特率" />
                  <ComboboxEmpty>请输入有效的波特率</ComboboxEmpty>
                  <ComboboxGroup class="max-h-52 overflow-y-auto">
                    <ComboboxItem v-for="rate in visibleBauds" :key="rate" :value="String(rate)">
                      {{ rate }}
                      <ComboboxItemIndicator><Check /></ComboboxItemIndicator>
                    </ComboboxItem>
                    <ComboboxItem v-if="customBaud !== null" :value="String(customBaud)">
                      使用 {{ customBaud }}
                      <ComboboxItemIndicator><Check /></ComboboxItemIndicator>
                    </ComboboxItem>
                  </ComboboxGroup>
                </ComboboxList>
              </Combobox>
            </Field>
            <div class="serial-format">
              <Select v-model="serial.dataBits" :disabled="locked">
                <SelectTrigger class="w-full min-w-0 gap-1 px-1.5" aria-label="数据位">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectGroup>
                    <SelectItem v-for="bits in [5, 6, 7, 8]" :key="bits" :value="bits">{{ bits }} 位</SelectItem>
                  </SelectGroup>
                </SelectContent>
              </Select>
              <Select v-model="serial.parity" :disabled="locked">
                <SelectTrigger class="w-full min-w-0 gap-1 px-1.5" aria-label="校验">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectGroup>
                    <SelectItem value="none">无校验</SelectItem>
                    <SelectItem value="odd">奇校验</SelectItem>
                    <SelectItem value="even">偶校验</SelectItem>
                    <SelectItem value="mark">标记校验</SelectItem>
                    <SelectItem value="space">空格校验</SelectItem>
                  </SelectGroup>
                </SelectContent>
              </Select>
              <Select v-model="serial.stopBits" :disabled="locked">
                <SelectTrigger class="w-full min-w-0 gap-1 px-1.5" aria-label="停止位">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectGroup>
                    <SelectItem :value="1">1 停止位</SelectItem>
                    <SelectItem :value="2">2 停止位</SelectItem>
                  </SelectGroup>
                </SelectContent>
              </Select>
            </div>
            <Field orientation="horizontal" class="field-row">
              <FieldLabel for="flow-control"><ArrowLeftRight />流控</FieldLabel>
              <Select v-model="serial.flowControl" :disabled="locked">
                <SelectTrigger id="flow-control" class="w-full min-w-0"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectGroup>
                    <SelectItem value="none">无流控</SelectItem>
                    <SelectItem value="hardware">硬件 RTS/CTS</SelectItem>
                    <SelectItem value="software">软件 XON/XOFF</SelectItem>
                  </SelectGroup>
                </SelectContent>
              </Select>
            </Field>
          </template>

          <template v-else-if="protocol === 'udp'">
            <Field orientation="horizontal" class="field-row">
              <FieldLabel for="udp-mode"><Globe2 />模式</FieldLabel>
              <Select v-model="udp.mode" :disabled="locked">
                <SelectTrigger id="udp-mode" class="w-full min-w-0"><SelectValue /></SelectTrigger>
                <SelectContent><SelectGroup>
                  <SelectItem value="unicast">单播</SelectItem>
                  <SelectItem value="broadcast">广播</SelectItem>
                  <SelectItem value="multicast">组播</SelectItem>
                </SelectGroup></SelectContent>
              </Select>
            </Field>
            <Field v-if="udp.mode === 'multicast'" orientation="horizontal" class="field-row">
              <FieldLabel for="udp-group"><Radio />组播地址</FieldLabel>
              <Input id="udp-group" v-model="udp.group" :disabled="locked" spellcheck="false" />
            </Field>
            <Field v-if="udp.mode === 'multicast'" orientation="horizontal" class="field-row">
              <FieldLabel for="udp-multicast-ttl"><Activity />组播 TTL</FieldLabel>
              <NumberField id="udp-multicast-ttl" v-model="udp.multicastTtl" :min="0" :max="255" :disabled="locked"
                :format-options="numberFormat" class="min-w-0" allow-invalid disable-wheel-change>
                <NumberFieldInput class="px-3 text-left" :aria-invalid="!validPort(udp.multicastTtl, 0) || Number(udp.multicastTtl) > 255" />
              </NumberField>
            </Field>
            <Field orientation="horizontal" class="field-row">
              <FieldLabel for="udp-local-host"><MapPin />本地地址</FieldLabel>
              <Input id="udp-local-host" v-model="udp.host" :disabled="locked" spellcheck="false" />
            </Field>
            <Field orientation="horizontal" class="field-row" :data-invalid="!validPort(udp.port, 0)">
              <FieldLabel for="udp-local-port"><Hash />本地端口</FieldLabel>
              <NumberField id="udp-local-port" v-model="udp.port" :min="0" :max="65535" :disabled="locked"
                :format-options="numberFormat" class="min-w-0" allow-invalid disable-wheel-change>
                <NumberFieldInput class="px-3 text-left" :aria-invalid="!validPort(udp.port, 0)" />
              </NumberField>
            </Field>
            <Field v-if="udp.mode !== 'multicast'" orientation="horizontal" class="field-row">
              <FieldLabel for="udp-remote-host"><TargetIcon />目的地址</FieldLabel>
              <Input id="udp-remote-host" v-model="udp.remoteHost" spellcheck="false" />
            </Field>
            <Field orientation="horizontal" class="field-row" :data-invalid="!validPort(udp.remotePort, 1)">
              <FieldLabel for="udp-remote-port"><Hash />目的端口</FieldLabel>
              <NumberField id="udp-remote-port" v-model="udp.remotePort" :min="1" :max="65535"
                :format-options="numberFormat" class="min-w-0" allow-invalid disable-wheel-change>
                <NumberFieldInput class="px-3 text-left" :aria-invalid="!validPort(udp.remotePort, 1)" />
              </NumberField>
            </Field>
          </template>

          <template v-else>
            <Field orientation="horizontal" class="field-row">
              <FieldLabel for="tcp-mode"><Globe2 />模式</FieldLabel>
              <Select v-model="tcp.mode" :disabled="locked">
                <SelectTrigger id="tcp-mode" class="w-full min-w-0"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectGroup>
                    <SelectItem value="tcpClient">客户端</SelectItem>
                    <SelectItem value="tcpServer">服务端</SelectItem>
                  </SelectGroup>
                </SelectContent>
              </Select>
            </Field>
            <Field orientation="horizontal" class="field-row">
              <FieldLabel for="tcp-host"><MapPin />{{ tcp.mode === 'tcpClient' ? '远端地址' : '监听地址' }}</FieldLabel>
              <Input id="tcp-host" v-model="activeAddress.host" :disabled="locked" spellcheck="false" />
            </Field>
            <Field orientation="horizontal" class="field-row" :data-invalid="!validPort(activeAddress.port, tcpMinPort)">
              <FieldLabel for="tcp-port"><Hash />{{ tcp.mode === 'tcpClient' ? '远端端口' : '监听端口' }}</FieldLabel>
              <NumberField id="tcp-port" v-model="activeAddress.port" :min="tcpMinPort" :max="65535" :disabled="locked"
                :format-options="numberFormat" class="min-w-0" allow-invalid disable-wheel-change>
                <NumberFieldInput class="px-3 text-left" :aria-invalid="!validPort(activeAddress.port, tcpMinPort)" />
              </NumberField>
            </Field>
            <template v-if="tcp.mode === 'tcpClient'">
              <Field orientation="horizontal" class="check-row">
                <Checkbox id="auto-reconnect" v-model="autoReconnect" :disabled="locked" />
                <FieldLabel for="auto-reconnect">自动重连</FieldLabel>
              </Field>
              <Field v-if="autoReconnect" orientation="horizontal" class="field-row">
                <FieldLabel for="reconnect-interval"><Activity />重连间隔</FieldLabel>
                <div class="flex min-w-0 items-center gap-2">
                  <NumberField id="reconnect-interval" v-model="reconnectInterval" :min="100" :max="2147483647"
                    :disabled="locked" :format-options="numberFormat" class="min-w-0 flex-1" allow-invalid disable-wheel-change>
                    <NumberFieldInput class="px-3 text-left" aria-label="重连间隔（毫秒）" />
                  </NumberField>
                  <span class="muted">ms</span>
                </div>
              </Field>
            </template>
            <Field v-if="tcp.mode === 'tcpServer' && connected" orientation="horizontal" class="field-row">
              <FieldLabel for="tcp-peer"><TargetIcon />发送目标</FieldLabel>
              <Select v-model="peer" :disabled="!clients.length">
                <SelectTrigger id="tcp-peer" class="w-full min-w-0">
                  <SelectValue :placeholder="clients.length ? '请选择客户端' : '等待客户端接入'" />
                </SelectTrigger>
                <SelectContent>
                  <SelectGroup>
                    <SelectItem v-for="client in clients" :key="client.id" :value="String(client.id)">{{ client.address }}</SelectItem>
                  </SelectGroup>
                </SelectContent>
              </Select>
            </Field>
          </template>

          <div class="connection-footer">
            <Badge variant="secondary" class="connection-status" :class="indicator.tone" role="status" :title="endpoint">
              <span class="status-dot" aria-hidden="true" />
              {{ indicator.label }}
            </Badge>
            <Button type="submit" :variant="connected || reconnecting ? 'outline' : 'default'" :disabled="(busy && !reconnecting) || (!connected && !reconnecting && !canOpen)">
              <Square v-if="connected || reconnecting" aria-hidden="true" />
              {{ actionLabel }}
            </Button>
          </div>
          <div v-if="connected && protocol !== 'serial'" class="connection-endpoint mono">{{ endpoint }}</div>
        </FieldGroup>
      </form>
    </Card>

    <Card class="panel frame-settings" role="region" aria-labelledby="frame-settings-title">
      <h2 id="frame-settings-title" class="panel-heading"><Settings aria-hidden="true" />帧设置</h2>
      <CardContent class="frame-form">
        <FieldGroup class="frame-switches">
          <Field orientation="horizontal" class="check-row">
            <Checkbox id="add-frame" v-model="addFrame" />
            <FieldLabel for="add-frame">发送加帧</FieldLabel>
          </Field>
          <Field orientation="horizontal" class="check-row">
            <Checkbox id="split-frames" v-model="splitFrames" />
            <FieldLabel for="split-frames">接收按帧拆分</FieldLabel>
          </Field>
        </FieldGroup>
        <div class="frame-mode">
          <span>帧格式</span>
          <ToggleGroup type="single" size="sm" :model-value="frameMode" aria-label="帧格式"
            @update:model-value="value => { if (value === 'hex' || value === 'text') frameMode = value }">
            <ToggleGroupItem value="text">TXT</ToggleGroupItem>
            <ToggleGroupItem value="hex">HEX</ToggleGroupItem>
          </ToggleGroup>
        </div>
        <FieldGroup class="frame-fields">
          <Field orientation="horizontal" class="field-row" :data-invalid="framingInvalid">
            <FieldLabel for="frame-header">帧头</FieldLabel>
            <Input id="frame-header" v-model="header" class="mono" :placeholder="frameMode === 'hex' ? 'AA 55' : '\\u0001'"
              spellcheck="false" :aria-invalid="framingInvalid" @blur="emit('applyFraming')" @keydown.enter.prevent="emit('applyFraming')" />
          </Field>
          <Field orientation="horizontal" class="field-row" :data-invalid="framingInvalid">
            <FieldLabel for="frame-tail">帧尾</FieldLabel>
            <Input id="frame-tail" v-model="tail" class="mono" :placeholder="frameMode === 'hex' ? '0D 0A' : '\\u0004'"
              spellcheck="false" :aria-invalid="framingInvalid" @blur="emit('applyFraming')" @keydown.enter.prevent="emit('applyFraming')" />
          </Field>
        </FieldGroup>
      </CardContent>
    </Card>
  </aside>
</template>
