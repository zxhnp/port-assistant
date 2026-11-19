import { execFile } from 'node:child_process'
import { fileURLToPath, URL } from 'node:url'
import tailwindcss from '@tailwindcss/vite'
import vue from '@vitejs/plugin-vue'
import { defineConfig, type Plugin } from 'vite'
import { localConnectionCheck } from './vite-local-connection'

const localSerialPorts: Plugin = {
  name: 'local-serial-ports',
  apply: 'serve',
  configureServer(server) {
    server.middlewares.use('/__dev/serial-ports', (request, response, next) => {
      if (request.method !== 'GET') return next()

      response.setHeader('Content-Type', 'application/json; charset=utf-8')
      response.setHeader('Cache-Control', 'no-store')
      if (process.platform !== 'win32') {
        response.end('[]')
        return
      }

      // 只枚举系统登记的设备名称，网页收发仍由模拟适配器处理。
      execFile('powershell.exe', [
        '-NoLogo', '-NoProfile', '-NonInteractive', '-Command',
        "$ErrorActionPreference = 'Stop'; [System.IO.Ports.SerialPort]::GetPortNames()",
      ], { windowsHide: true, timeout: 4000 }, (error, stdout) => {
        if (error) {
          response.statusCode = 503
          response.end('[]')
          return
        }

        const names = stdout.split(/\r?\n/).map(name => name.trim()).filter(Boolean)
        const ports = [...new Set(names)].sort((a, b) => a.localeCompare(b, 'en', { numeric: true }))
        response.end(JSON.stringify(ports))
      })
    })
  },
}

export default defineConfig({
  plugins: [vue(), tailwindcss(), localSerialPorts, localConnectionCheck],
  resolve: { alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) } },
  clearScreen: false,
  server: { host: '127.0.0.1', port: 1420, strictPort: true },
})
