import { execFile } from 'node:child_process'
import type { IncomingMessage, ServerResponse } from 'node:http'
import type { Plugin } from 'vite'
import type { Connection } from './src/lib/transport'

const checkScript = `
$ErrorActionPreference = 'Stop'
[Console]::InputEncoding = [Text.UTF8Encoding]::new($false)
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$options = [Console]::In.ReadToEnd() | ConvertFrom-Json
$handle = $null
try {
  switch ($options.kind) {
    'serial' {
      $port = [string]$options.port
      if ([IO.Ports.SerialPort]::GetPortNames() -notcontains $port) {
        throw [IO.FileNotFoundException]::new('The serial port is not present.')
      }
      $handle = [IO.Ports.SerialPort]::new($port, [int]$options.baudRate,
        [IO.Ports.Parity]$options.parity, [int]$options.dataBits, [IO.Ports.StopBits]$options.stopBits)
      $handshake = @{ none = 'None'; hardware = 'RequestToSend'; software = 'XOnXOff' }
      $handle.Handshake = [IO.Ports.Handshake]$handshake[$options.flowControl]
      $handle.Open()
      $endpoint = $port
    }
    'tcpClient' {
      $handle = [Net.Sockets.TcpClient]::new([string]$options.remote.host, [int]$options.remote.port)
      $endpoint = $handle.Client.RemoteEndPoint.ToString()
    }
    default {
      $lastError = $null
      $parsed = $null
      $addresses = if ([Net.IPAddress]::TryParse([string]$options.local.host, [ref]$parsed)) {
        @($parsed)
      } else {
        [Net.Dns]::GetHostAddresses([string]$options.local.host)
      }
      foreach ($ip in $addresses) {
        try {
          $udp = $options.kind -eq 'udp'
          $socketType = if ($udp) { [Net.Sockets.SocketType]::Dgram } else { [Net.Sockets.SocketType]::Stream }
          $protocol = if ($udp) { [Net.Sockets.ProtocolType]::Udp } else { [Net.Sockets.ProtocolType]::Tcp }
          $handle = [Net.Sockets.Socket]::new($ip.AddressFamily, $socketType, $protocol)
          $handle.ExclusiveAddressUse = $true
          $handle.Bind([Net.IPEndPoint]::new($ip, [int]$options.local.port))
          if (-not $udp) { $handle.Listen(1) }
          $endpoint = $handle.LocalEndPoint.ToString()
          break
        } catch {
          $lastError = $_.Exception
          if ($null -ne $handle) { $handle.Dispose(); $handle = $null }
        }
      }
      if (-not $endpoint) {
        if ($lastError) { throw $lastError }
        throw [IO.IOException]::new('No local IP address resolved.')
      }
    }
  }
  $result = @{ endpoint = $endpoint }
} catch {
  $cause = $_.Exception
  while ($cause.InnerException) { $cause = $cause.InnerException }
  $code = if ($cause -is [Net.Sockets.SocketException]) {
    $cause.SocketErrorCode.ToString()
  } elseif ($cause -is [UnauthorizedAccessException]) {
    'AccessDenied'
  } elseif ($cause -is [IO.FileNotFoundException]) {
    'SerialNotFound'
  } else {
    'Unavailable'
  }
  $result = @{ code = $code; detail = $cause.Message }
} finally {
  if ($null -ne $handle) { $handle.Dispose() }
}
[Console]::Write(($result | ConvertTo-Json -Compress))
`

type ProbeResult = { endpoint: string } | { code: string; detail: string }

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function integer(value: unknown, min: number, max: number): value is number {
  return typeof value === 'number' && Number.isInteger(value) && value >= min && value <= max
}

function address(value: unknown, minPort: number) {
  return record(value) && typeof value.host === 'string' && value.host.length <= 255
    && !!value.host.trim() && integer(value.port, minPort, 65_535)
}

function connection(value: unknown): value is Connection {
  if (!record(value)) return false
  switch (value.kind) {
    case 'serial':
      return typeof value.port === 'string' && value.port.length <= 64 && !!value.port.trim()
        && integer(value.baudRate, 1, 2_147_483_647) && integer(value.dataBits, 5, 8)
        && integer(value.stopBits, 1, 2)
        && typeof value.parity === 'string' && ['none', 'odd', 'even', 'mark', 'space'].includes(value.parity)
        && typeof value.flowControl === 'string' && ['none', 'hardware', 'software'].includes(value.flowControl)
    case 'udp':
    case 'tcpServer':
      return address(value.local, 0)
    case 'tcpClient':
      return address(value.remote, 1)
    default:
      return false
  }
}

function json(response: ServerResponse, status: number, payload: unknown) {
  if (response.destroyed) return
  response.writeHead(status, { 'Content-Type': 'application/json; charset=utf-8', 'Cache-Control': 'no-store' })
  response.end(JSON.stringify(payload))
}

function isLocalRequest(request: IncomingMessage) {
  const remote = request.socket.remoteAddress
  if (remote !== '127.0.0.1' && remote !== '::1' && remote !== '::ffff:127.0.0.1') return false
  const { host, origin } = request.headers
  return !origin || origin === `http://${host}`
}

async function readConnection(request: IncomingMessage) {
  const chunks: Buffer[] = []
  let size = 0
  for await (const chunk of request) {
    size += chunk.length
    if (size > 4096) throw new Error('请求内容过长')
    chunks.push(Buffer.from(chunk))
  }
  const value: unknown = JSON.parse(Buffer.concat(chunks).toString('utf8'))
  if (!connection(value)) throw new Error('连接参数无效')
  return value
}

function probe(options: Connection) {
  return new Promise<ProbeResult>((resolve, reject) => {
    // 参数通过标准输入传递，PowerShell 命令本身不包含来自页面的内容。
    const child = execFile('powershell.exe', ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command', checkScript],
      { windowsHide: true, timeout: 5500, maxBuffer: 32 * 1024 }, (error, stdout) => {
        if (error) {
          reject(new Error(error.killed ? '本机连接检查超时' : '无法运行 Windows 连接检查'))
          return
        }
        try {
          resolve(JSON.parse(stdout) as ProbeResult)
        } catch {
          reject(new Error('无法读取 Windows 连接检查结果'))
        }
      })
    child.stdin?.on('error', () => {})
    child.stdin?.end(JSON.stringify(options), 'utf8')
  })
}

function addressLabel(host: string, port: number) {
  return `${host.includes(':') ? `[${host}]` : host}:${port}`
}

function describeFailure(options: Connection, failure: Exclude<ProbeResult, { endpoint: string }>) {
  const { code, detail } = failure
  if (options.kind === 'serial') {
    if (code === 'AccessDenied') return `串口 ${options.port} 打开失败，可能已被占用或当前进程无访问权限`
    if (code === 'SerialNotFound') return `未发现串口 ${options.port}，请刷新串口列表`
    return `无法打开串口 ${options.port}：${detail}`
  }
  if (options.kind === 'tcpClient') {
    const remote = addressLabel(options.remote.host, options.remote.port)
    if (code === 'ConnectionRefused') return `TCP 连接失败，远端 ${remote} 拒绝连接`
    return `无法连接 TCP 远端 ${remote}：${detail}`
  }
  const target = addressLabel(options.local.host, options.local.port)
  const protocol = options.kind === 'udp' ? 'UDP' : 'TCP'
  if (code === 'AddressAlreadyInUse') return `${protocol} 本地地址 ${target} 已被占用`
  if (code === 'AccessDenied') return `无法绑定 ${protocol} 本地地址 ${target}，端口可能已被占用或受系统限制`
  return `无法绑定 ${protocol} 本地地址 ${target}：${detail}`
}

export const localConnectionCheck: Plugin = {
  name: 'local-connection-check',
  apply: 'serve',
  configureServer(server) {
    server.middlewares.use('/__dev/check-connection', (request, response, next) => {
      if (request.method !== 'POST') return next()
      if (!isLocalRequest(request)) return json(response, 403, { error: '只允许从本机开发页面检查连接' })
      if (!request.headers['content-type']?.startsWith('application/json')) {
        return json(response, 415, { error: '连接检查需要 JSON 参数' })
      }
      if (process.platform !== 'win32') return json(response, 503, { error: '本机连接检查需要 Windows' })

      void readConnection(request).then(async options => {
        try {
          const result = await probe(options)
          if ('endpoint' in result) return json(response, 200, result)
          json(response, 409, { error: describeFailure(options, result) })
        } catch (error) {
          json(response, 503, { error: error instanceof Error ? error.message : '本机连接检查失败' })
        }
      }, () => json(response, 400, { error: '连接参数无效' }))
    })
  },
}
