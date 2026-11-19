<script setup lang="ts">
import { computed } from 'vue'
import type { ConnectionIndicator } from '@/composables/useWorkbench'
import type { Connection } from '@/lib/transport'

const props = defineProps<{
  connection: Connection | null
  indicator: ConnectionIndicator
  endpoint: string
  clientCount: number
}>()

const parityLabels = { none: '无', odd: '奇', even: '偶', mark: '标记', space: '空格' }
const udpModeLabels = { unicast: '单播', broadcast: '广播', multicast: '组播' }

const fields = computed(() => {
  const { indicator, connection, endpoint, clientCount } = props
  if (indicator.tone !== 'online' || !connection) return []

  switch (connection.kind) {
    case 'serial':
      return [
        { label: '串口', value: endpoint || connection.port },
        { label: '波特率', value: connection.baudRate },
        { label: '数据位', value: connection.dataBits },
        { label: '校验', value: parityLabels[connection.parity] },
        { label: '停止位', value: connection.stopBits },
      ]
    case 'udp':
      return [{ label: 'UDP 模式', value: udpModeLabels[connection.mode ?? 'unicast'] }, { label: '本地地址', value: endpoint }]
    case 'tcpClient':
      return [{ label: 'TCP 模式', value: '客户端' }, { label: '远端地址', value: endpoint }]
    case 'tcpServer':
      return [
        { label: 'TCP 模式', value: '服务端' },
        { label: '监听地址', value: endpoint },
        { label: '客户端', value: clientCount },
      ]
  }
})
</script>

<template>
  <footer class="connection-bar" aria-label="连接状态栏">
    <span class="connection-bar-state" :class="indicator.tone" role="status">
      <span class="status-dot" aria-hidden="true" />
      {{ indicator.label }}
    </span>
    <dl v-if="fields.length" class="connection-bar-fields">
      <div v-for="{ label, value } in fields" :key="label">
        <dt>{{ label }}</dt>
        <dd>{{ value }}</dd>
      </div>
    </dl>
  </footer>
</template>
