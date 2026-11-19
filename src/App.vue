<script setup lang="ts">
import { onBeforeUnmount, onMounted, shallowRef, useTemplateRef } from 'vue'
import { Copy, Maximize2, Minus, MoonStar, PanelsTopLeft, Pin, PinOff, Sun, X } from '@lucide/vue'
import appIcon from '@/assets/app-icon.svg'
import { invoke } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { Message } from '@/lib/message'
import ConnectionPanel from '@/components/ConnectionPanel.vue'
import ConnectionStatus from '@/components/ConnectionStatus.vue'
import ReceivePanel from '@/components/ReceivePanel.vue'
import SendPanel from '@/components/SendPanel.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Toaster } from '@/components/ui/sonner'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'
import { useWorkbench } from '@/composables/useWorkbench'
import type { Protocol } from '@/lib/transport'
import { transport } from '@/lib/transport'
import 'vue-sonner/style.css'

const workbench = useWorkbench()
const {
  connected, reconnecting, autoReconnect, reconnectIntervalMs, activeConnection, connectionIndicator, endpoint, clients, ports, busy, refreshing, sending, timerBusy, running,
  pinned, pinBusy, target, input, sendMode, timed, intervalMs, addFrame, splitFrames,
  frameMode, headerInput, tailInput, paused, visible, trafficStats, framing, packet, validInterval, canSend,
} = workbench

const protocol = shallowRef<Protocol>('serial')
const showTime = shallowRef(true)
const autoScroll = shallowRef(true)
const workspace = useTemplateRef<HTMLElement>('workspace')
let workspaceObserver: ResizeObserver | undefined
const maximized = shallowRef(false)
let unlistenResize: UnlistenFn | undefined
let disposed = false

async function windowAction(action: 'minimize' | 'toggleMaximize' | 'close') {
  try {
    await getCurrentWindow()[action]()
  } catch (error) {
    Message.error(error)
  }
}

async function openNewInstance() {
  try {
    await invoke('open_new_instance')
  } catch (error) {
    Message.error(error)
  }
}

async function watchWindowState() {
  const window = getCurrentWindow()
  try {
    maximized.value = await window.isMaximized()
    const unlisten = await window.onResized(async () => {
      maximized.value = await window.isMaximized()
    })
    if (disposed) unlisten()
    else unlistenResize = unlisten
  } catch (error) {
    if (!disposed) Message.error(error)
  }
}

const colorPreference = window.matchMedia('(prefers-color-scheme: dark)')
const storageKey = 'port-assistant.theme'
const remembered = localStorage.getItem(storageKey)
let themeSelected = remembered === 'dark' || remembered === 'light'
const dark = shallowRef(remembered === 'dark' || (!themeSelected && colorPreference.matches))

function setTheme(value: boolean) {
  dark.value = value
  document.documentElement.classList.toggle('dark', value)
  document.documentElement.style.colorScheme = value ? 'dark' : 'light'
}

function toggleTheme() {
  themeSelected = true
  setTheme(!dark.value)
  localStorage.setItem(storageKey, dark.value ? 'dark' : 'light')
}

function matchSystem(event: MediaQueryListEvent) {
  if (!themeSelected) setTheme(event.matches)
}

setTheme(dark.value)
onMounted(() => {
  const container = workspace.value
  const sendInput = container?.querySelector<HTMLElement>('.send-input')
  if (container && sendInput) {
    workspaceObserver = new ResizeObserver(() => {
      const bottom = container.getBoundingClientRect().bottom - sendInput.getBoundingClientRect().bottom
      container.style.setProperty('--detail-bottom', `${Math.max(0, bottom)}px`)
    })
    workspaceObserver.observe(container)
    workspaceObserver.observe(sendInput)
    const sendPanel = container.querySelector<HTMLElement>('.send-panel')
    if (sendPanel) workspaceObserver.observe(sendPanel)
  }
  if (!transport.simulated) void watchWindowState()
  colorPreference.addEventListener('change', matchSystem)
  void workbench.refreshPorts()
})
onBeforeUnmount(() => {
  disposed = true
  unlistenResize?.()
  workspaceObserver?.disconnect()
  colorPreference.removeEventListener('change', matchSystem)
})
</script>

<template>
  <TooltipProvider :delay-duration="350">
    <div class="app-shell">
      <header class="menu-bar" data-tauri-drag-region>
        <div class="app-name" data-tauri-drag-region><img class="app-icon" :src="appIcon" alt="" data-tauri-drag-region /><h1 data-tauri-drag-region>端口助手</h1></div>
        <div class="title-drag-region" data-tauri-drag-region aria-hidden="true" />
        <div class="window-actions">
          <Badge v-if="transport.simulated" variant="outline" class="mock-badge" title="网页仅模拟收发。实际串口、UDP 和 TCP 收发请使用 Windows 桌面版。">网页模拟</Badge>
          <Tooltip v-if="!transport.simulated">
            <TooltipTrigger as-child>
              <Button variant="ghost" size="icon-sm" aria-label="打开新窗口" @click="openNewInstance"><PanelsTopLeft /></Button>
            </TooltipTrigger>
            <TooltipContent>打开新窗口</TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger as-child>
              <Button :variant="pinned ? 'secondary' : 'ghost'" size="icon-sm" class="pin-button"
                :aria-pressed="pinned" :disabled="pinBusy" :aria-label="pinned ? '取消置顶' : '窗口置顶'" @click="workbench.togglePin">
                <PinOff v-if="pinned" /><Pin v-else />
              </Button>
            </TooltipTrigger>
            <TooltipContent>{{ pinned ? '取消置顶' : '窗口置顶' }}</TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger as-child>
              <Button variant="ghost" size="icon-sm" :aria-label="dark ? '切换到浅色' : '切换到深色'" @click="toggleTheme">
                <Sun v-if="dark" /><MoonStar v-else />
              </Button>
            </TooltipTrigger>
            <TooltipContent>{{ dark ? '切换到浅色' : '切换到深色' }}</TooltipContent>
          </Tooltip>
          <div v-if="!transport.simulated" class="window-controls">
            <Button variant="ghost" size="icon-sm" aria-label="最小化" @click="windowAction('minimize')"><Minus /></Button>
            <Button variant="ghost" size="icon-sm" :aria-label="maximized ? '还原窗口' : '最大化'"
              @click="windowAction('toggleMaximize')"><Copy v-if="maximized" /><Maximize2 v-else /></Button>
            <Button variant="ghost" size="icon-sm" class="window-close" aria-label="关闭窗口" @click="windowAction('close')"><X /></Button>
          </div>
        </div>
      </header>

      <main class="workbench">
        <ConnectionPanel v-model:protocol="protocol" v-model:target="target"
          v-model:auto-reconnect="autoReconnect" v-model:reconnect-interval="reconnectIntervalMs"
          v-model:add-frame="addFrame" v-model:split-frames="splitFrames" :frame-mode="frameMode" v-model:header="headerInput" v-model:tail="tailInput"
          @update:frame-mode="workbench.setFrameMode"
          :ports="ports" :clients="clients" :busy="busy" :connected="connected" :reconnecting="reconnecting" :indicator="connectionIndicator" :refreshing="refreshing"
          :endpoint="endpoint" :framing-invalid="(addFrame || splitFrames) && !!framing.error"
          @connect="workbench.connect" @disconnect="workbench.disconnect" @refresh="workbench.refreshPorts" @apply-framing="workbench.applyFraming" />

        <div ref="workspace" class="workspace">
          <ReceivePanel v-model:paused="paused" v-model:show-time="showTime" v-model:auto-scroll="autoScroll"
            :rows="visible" :connected="connected" :frame-mode="frameMode" @clear="workbench.clear" />
          <SendPanel v-model:input="input" v-model:mode="sendMode" v-model:timed="timed" v-model:interval="intervalMs"
            :frame-mode="frameMode" :packet="packet.value" :input-error="addFrame && !framing.value ? '' : packet.error" :can-send="canSend" :sending="sending" :running="running"
            :timer-busy="timerBusy" :valid-interval="validInterval" @send="workbench.send" @toggle-timer="workbench.toggleTimer" />
        </div>
      </main>

      <ConnectionStatus :connection="activeConnection" :indicator="connectionIndicator" :endpoint="endpoint" :client-count="clients.length"
        :traffic-stats="trafficStats" @reset-traffic-stats="workbench.resetTrafficStats" />

    </div>
  </TooltipProvider>

  <Toaster class="message-toaster" position="top-center" :offset="{ top: 72 }" :mobile-offset="{ top: 68 }"
    :theme="dark ? 'dark' : 'light'" :visible-toasts="3" :gap="16" container-aria-label="消息提示"
    :toast-options="{ unstyled: true }" expand />
</template>
