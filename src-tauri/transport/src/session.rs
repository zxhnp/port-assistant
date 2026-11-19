use crate::frame::{Decoder, raw_packet};
use crate::{
    Address, Connection, Direction, Event, FlowControl, Framing, Packet, Parity, Peer, Record,
    Target, UdpMode,
};
use std::collections::BTreeMap;
use std::future::pending;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket, lookup_host};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep_until, timeout};
use tokio_serial::SerialPortBuilderExt;

const IO_TIMEOUT: Duration = Duration::from_secs(5);
const INBOX_SIZE: usize = 32;
const MAX_PEERS: usize = 32;
const MAX_UDP_PAYLOAD: usize = 65_507;

pub struct Session {
    commands: mpsc::Sender<Command>,
    finished: oneshot::Receiver<()>,
}

impl Session {
    pub fn is_finished(&self) -> bool {
        self.commands.is_closed()
    }

    pub async fn set_framing(&self, framing: Option<Framing>) -> Result<(), String> {
        if let Some(framing) = &framing {
            framing.validate()?;
        }
        self.request(|done| Command::SetFraming(framing, done))
            .await
    }

    pub async fn send(&self, packet: Packet, target: Option<Target>) -> Result<(), String> {
        packet.validate()?;
        self.request(|done| Command::Send {
            packet,
            target,
            interval: None,
            done,
        })
        .await
    }

    pub async fn start_timer(
        &self,
        packet: Packet,
        target: Option<Target>,
        interval_ms: u32,
    ) -> Result<(), String> {
        packet.validate()?;
        if interval_ms == 0 {
            return Err("定时间隔至少为 1 毫秒".into());
        }
        self.request(|done| Command::Send {
            packet,
            target,
            interval: Some(Duration::from_millis(interval_ms.into())),
            done,
        })
        .await
    }

    pub async fn stop_timer(&self) -> Result<(), String> {
        self.request(Command::StopTimer).await
    }

    pub async fn close(self) {
        let _ = self.commands.send(Command::Close).await;
        let _ = self.finished.await;
    }

    async fn request(&self, make: impl FnOnce(Reply) -> Command) -> Result<(), String> {
        let (sender, receiver) = oneshot::channel();
        self.commands
            .send(make(sender))
            .await
            .map_err(|_| disconnected())?;
        receiver.await.map_err(|_| disconnected())?
    }
}

fn disconnected() -> String {
    "连接已关闭".into()
}

type Reply = oneshot::Sender<Result<(), String>>;
type Writer = Box<dyn AsyncWrite + Send + Unpin>;

enum Command {
    SetFraming(Option<Framing>, Reply),
    Send {
        packet: Packet,
        target: Option<Target>,
        interval: Option<Duration>,
        done: Reply,
    },
    StopTimer(Reply),
    Close,
}

enum Incoming {
    Accepted(TcpStream, SocketAddr),
    Bytes {
        id: u64,
        bytes: Vec<u8>,
        source: Option<SocketAddr>,
    },
    Ended {
        id: u64,
        error: Option<String>,
    },
    SocketError(String),
}

struct Task(JoinHandle<()>);

impl Task {
    async fn stop(mut self) {
        self.0.abort();
        let _ = (&mut self.0).await;
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct StreamPeer {
    label: String,
    write: Writer,
    decoder: Option<Decoder>,
    _reader: Task,
}

impl StreamPeer {
    async fn close(self) {
        drop(self.write);
        self._reader.stop().await;
    }
}

enum Route {
    Stream(u64),
    Udp(SocketAddr),
}

struct Scheduled {
    packet: Packet,
    route: Route,
    interval: Duration,
    next: Instant,
}

struct Actor<F> {
    emit: F,
    endpoint: String,
    server: bool,
    peers: BTreeMap<u64, StreamPeer>,
    next_peer: u64,
    framing: Option<Framing>,
    udp: Option<Arc<UdpSocket>>,
    timer: Option<Scheduled>,
    input: mpsc::Sender<Incoming>,
    _socket_task: Option<Task>,
    alive: bool,
}

pub async fn connect(
    connection: Connection,
    framing: Option<Framing>,
    emit: impl Fn(Event) + Send + 'static,
) -> Result<Session, String> {
    if let Some(framing) = &framing {
        framing.validate()?;
    }
    let (incoming, incoming_rx) = mpsc::channel(INBOX_SIZE);
    let mut actor = Actor {
        emit,
        endpoint: String::new(),
        server: false,
        peers: BTreeMap::new(),
        next_peer: 1,
        framing,
        udp: None,
        timer: None,
        input: incoming,
        _socket_task: None,
        alive: true,
    };

    let open = actor.open(connection);
    timeout(IO_TIMEOUT, open)
        .await
        .map_err(|_| "连接超时".to_string())??;
    let (commands, command_rx) = mpsc::channel(INBOX_SIZE);
    let (finished_tx, finished) = oneshot::channel();
    tokio::spawn(async move {
        actor.run(command_rx, incoming_rx).await;
        let _ = finished_tx.send(());
    });
    Ok(Session { commands, finished })
}

impl<F: Fn(Event)> Actor<F> {
    async fn open(&mut self, connection: Connection) -> Result<(), String> {
        match connection {
            Connection::Serial {
                port,
                baud_rate,
                data_bits,
                parity,
                stop_bits,
                flow_control,
            } => {
                use tokio_serial::{DataBits, StopBits};
                if port.trim().is_empty() || baud_rate == 0 {
                    return Err("请选择串口并填写有效的波特率".into());
                }
                let data_bits = match data_bits {
                    5 => DataBits::Five,
                    6 => DataBits::Six,
                    7 => DataBits::Seven,
                    8 => DataBits::Eight,
                    _ => return Err("数据位须为 5～8".into()),
                };
                let stop_bits = match stop_bits {
                    1 => StopBits::One,
                    2 => StopBits::Two,
                    _ => return Err("停止位须为 1 或 2".into()),
                };
                let standard_parity = match &parity {
                    Parity::None | Parity::Mark | Parity::Space => tokio_serial::Parity::None,
                    Parity::Odd => tokio_serial::Parity::Odd,
                    Parity::Even => tokio_serial::Parity::Even,
                };
                let flow_control = match flow_control {
                    FlowControl::None => tokio_serial::FlowControl::None,
                    FlowControl::Software => tokio_serial::FlowControl::Software,
                    FlowControl::Hardware => tokio_serial::FlowControl::Hardware,
                };
                let stream = tokio_serial::new(&port, baud_rate)
                    .data_bits(data_bits)
                    .stop_bits(stop_bits)
                    .parity(standard_parity)
                    .flow_control(flow_control)
                    .open_native_async()
                    .map_err(|error| format!("无法打开串口 {port}：{error}"))?;
                apply_extended_parity(&stream, &parity)
                    .map_err(|error| format!("串口 {port} 校验设置失败：{error}"))?;
                self.endpoint = port.clone();
                self.add_stream(0, port, stream);
            }
            Connection::Udp {
                local,
                mode,
                group,
                interface,
                multicast_ttl,
            } => {
                let socket = UdpSocket::bind(host_tuple(&local)?)
                    .await
                    .map_err(|error| format!("无法绑定 UDP：{error}"))?;
                match mode {
                    UdpMode::Unicast => {}
                    UdpMode::Broadcast => {
                        if !socket
                            .local_addr()
                            .map_err(|error| error.to_string())?
                            .is_ipv4()
                        {
                            return Err("UDP 广播需要 IPv4 本地地址".into());
                        }
                        socket
                            .set_broadcast(true)
                            .map_err(|error| format!("无法启用 UDP 广播：{error}"))?;
                    }
                    UdpMode::Multicast => {
                        if !socket
                            .local_addr()
                            .map_err(|error| error.to_string())?
                            .is_ipv4()
                        {
                            return Err("UDP 组播需要 IPv4 本地地址".into());
                        }
                        let group: Ipv4Addr = group
                            .as_deref()
                            .unwrap_or("")
                            .parse()
                            .map_err(|_| "请输入有效的 IPv4 组播地址".to_string())?;
                        if !group.is_multicast() {
                            return Err("组播地址须在 224.0.0.0～239.255.255.255 范围内".into());
                        }
                        let local_address =
                            socket.local_addr().map_err(|error| error.to_string())?;
                        let interface: Ipv4Addr = match interface {
                            Some(address) => address
                                .parse()
                                .map_err(|_| "请输入有效的 IPv4 网卡地址".to_string())?,
                            None => match local_address {
                                SocketAddr::V4(address) => *address.ip(),
                                SocketAddr::V6(_) => {
                                    return Err("UDP 组播需要 IPv4 本地地址".into());
                                }
                            },
                        };
                        socket
                            .set_multicast_ttl_v4(u32::from(multicast_ttl.unwrap_or(1)))
                            .map_err(|error| format!("设置 UDP 组播 TTL 失败：{error}"))?;
                        socket
                            .join_multicast_v4(group, interface)
                            .map_err(|error| format!("加入 UDP 组播失败：{error}"))?;
                        socket2::SockRef::from(&socket)
                            .set_multicast_if_v4(&interface)
                            .map_err(|error| format!("设置 UDP 组播网卡失败：{error}"))?;
                    }
                }
                self.endpoint = socket
                    .local_addr()
                    .map_err(|error| error.to_string())?
                    .to_string();
                let socket = Arc::new(socket);
                let reader = socket.clone();
                let tx = self.input.clone();
                self._socket_task = Some(Task(tokio::spawn(async move {
                    let mut buffer = vec![0; 65_536];
                    loop {
                        match reader.recv_from(&mut buffer).await {
                            Ok((len, source)) => {
                                let event = Incoming::Bytes {
                                    id: 0,
                                    bytes: buffer[..len].to_vec(),
                                    source: Some(source),
                                };
                                if tx.send(event).await.is_err() {
                                    break;
                                }
                            }
                            Err(error) => {
                                let _ = tx
                                    .send(Incoming::SocketError(format!("UDP 接收失败：{error}")))
                                    .await;
                                break;
                            }
                        }
                    }
                })));
                self.udp = Some(socket);
            }
            Connection::TcpClient { remote } => {
                if remote.port == 0 {
                    return Err("远端端口须为 1～65535".into());
                }
                let stream = TcpStream::connect(host_tuple(&remote)?)
                    .await
                    .map_err(|error| format!("TCP 连接失败：{error}"))?;
                let _ = stream.set_nodelay(true);
                self.endpoint = stream
                    .peer_addr()
                    .map_err(|error| error.to_string())?
                    .to_string();
                self.add_stream(0, self.endpoint.clone(), stream);
            }
            Connection::TcpServer { local } => {
                let listener = TcpListener::bind(host_tuple(&local)?)
                    .await
                    .map_err(|error| format!("TCP 监听失败：{error}"))?;
                self.endpoint = listener
                    .local_addr()
                    .map_err(|error| error.to_string())?
                    .to_string();
                self.server = true;
                let tx = self.input.clone();
                self._socket_task = Some(Task(tokio::spawn(async move {
                    loop {
                        match listener.accept().await {
                            Ok((socket, address)) => {
                                if tx.send(Incoming::Accepted(socket, address)).await.is_err() {
                                    break;
                                }
                            }
                            Err(error) => {
                                let _ = tx
                                    .send(Incoming::SocketError(format!("TCP 监听失败：{error}")))
                                    .await;
                                break;
                            }
                        }
                    }
                })));
            }
        }
        Ok(())
    }

    fn add_stream<S: AsyncRead + AsyncWrite + Send + Unpin + 'static>(
        &mut self,
        id: u64,
        label: String,
        stream: S,
    ) {
        let (mut reader, writer) = tokio::io::split(stream);
        let tx = self.input.clone();
        let reader = Task(tokio::spawn(async move {
            let mut buffer = vec![0; 8 * 1024];
            let error = loop {
                match reader.read(&mut buffer).await {
                    Ok(0) => break None,
                    Ok(len) => {
                        let event = Incoming::Bytes {
                            id,
                            bytes: buffer[..len].to_vec(),
                            source: None,
                        };
                        if tx.send(event).await.is_err() {
                            return;
                        }
                    }
                    Err(error) => break Some(format!("接收失败：{error}")),
                }
            };
            let _ = tx.send(Incoming::Ended { id, error }).await;
        }));
        self.peers.insert(
            id,
            StreamPeer {
                label,
                write: Box::new(writer),
                _reader: reader,
                decoder: self.framing.as_ref().map(Decoder::new),
            },
        );
    }

    async fn run(
        mut self,
        mut commands: mpsc::Receiver<Command>,
        mut incoming: mpsc::Receiver<Incoming>,
    ) {
        self.status();
        while self.alive {
            let next = self.timer.as_ref().map(|timer| timer.next);
            tokio::select! {
                biased;
                command = commands.recv() => {
                    match command {
                        Some(Command::Close) | None => break,
                        Some(command) => self.command(command).await,
                    }
                }
                () = wait_for_tick(next) => self.tick().await,
                message = incoming.recv() => {
                    if let Some(message) = message { self.receive(message).await; }
                }
            }
        }
        commands.close();
        self.stop_timer();
        for (_, peer) in std::mem::take(&mut self.peers) {
            peer.close().await;
        }
        if let Some(task) = self._socket_task.take() {
            task.stop().await;
        }
        self.udp.take();
        self.alive = false;
        self.status();
    }

    fn status(&self) {
        let clients = if self.server {
            self.peers
                .iter()
                .map(|(id, peer)| Peer {
                    id: *id,
                    address: peer.label.clone(),
                })
                .collect()
        } else {
            Vec::new()
        };
        (self.emit)(Event::Status {
            connected: self.alive,
            endpoint: self.endpoint.clone(),
            clients,
        });
    }

    async fn command(&mut self, command: Command) {
        match command {
            Command::SetFraming(framing, done) => {
                // 规则变更后，旧规则下积累的半帧随解码器一起清除。
                self.framing = framing;
                for peer in self.peers.values_mut() {
                    peer.decoder = self.framing.as_ref().map(Decoder::new);
                }
                let _ = done.send(Ok(()));
            }
            Command::Send {
                packet,
                target,
                interval,
                done,
            } => {
                let result = self.prepare_send(packet, target, interval).await;
                let _ = done.send(result);
            }
            Command::StopTimer(done) => {
                self.stop_timer();
                let _ = done.send(Ok(()));
            }
            Command::Close => unreachable!("关闭命令由事件循环处理"),
        }
    }

    async fn prepare_send(
        &mut self,
        packet: Packet,
        target: Option<Target>,
        interval: Option<Duration>,
    ) -> Result<(), String> {
        if interval.is_some() && self.timer.is_some() {
            return Err("定时发送正在运行".into());
        }
        let route = match target {
            Some(Target::Udp { host, port }) if self.udp.is_some() => {
                if packet.bytes.len() > MAX_UDP_PAYLOAD {
                    return Err("UDP 单条消息最多支持 65507 字节".into());
                }
                if host.trim().is_empty() || port == 0 {
                    return Err("请填写 UDP 目的地址和端口".into());
                }
                let local = self
                    .udp
                    .as_ref()
                    .unwrap()
                    .local_addr()
                    .map_err(|error| error.to_string())?;
                let addresses = timeout(IO_TIMEOUT, lookup_host((host.trim(), port)))
                    .await
                    .map_err(|_| "解析 UDP 目的地址超时".to_string())?
                    .map_err(|error| format!("无法解析 UDP 目的地址：{error}"))?;
                let address = addresses
                    .into_iter()
                    .find(|address| address.is_ipv4() == local.is_ipv4())
                    .ok_or_else(|| "UDP 目的地址与本地地址的 IP 版本不一致".to_string())?;
                Route::Udp(address)
            }
            Some(Target::Peer { id }) if self.server && self.peers.contains_key(&id) => {
                Route::Stream(id)
            }
            None if self.peers.contains_key(&0) => Route::Stream(0),
            _ => return Err("请选择有效的发送目标".into()),
        };

        if let Some(interval) = interval {
            self.timer = Some(Scheduled {
                packet,
                route,
                interval,
                next: Instant::now(),
            });
            (self.emit)(Event::Timer { running: true });
            Ok(())
        } else {
            self.write(&route, &packet).await
        }
    }

    async fn write(&mut self, route: &Route, packet: &Packet) -> Result<(), String> {
        let (result, label) = match route {
            Route::Stream(id) => {
                let peer = self.peers.get_mut(id).ok_or_else(disconnected)?;
                let result = timeout(IO_TIMEOUT, peer.write.write_all(&packet.bytes)).await;
                (result, peer.label.clone())
            }
            Route::Udp(address) => {
                let socket = self.udp.as_ref().ok_or_else(disconnected)?;
                let result = timeout(IO_TIMEOUT, async {
                    let sent = socket.send_to(&packet.bytes, address).await?;
                    if sent != packet.bytes.len() {
                        return Err(std::io::ErrorKind::WriteZero.into());
                    }
                    Ok(())
                })
                .await;
                (result, address.to_string())
            }
        };
        let error = match result {
            Ok(Ok(())) => {
                (self.emit)(Event::Messages {
                    rows: vec![Record::new(Direction::Tx, Some(label), packet.clone())],
                });
                return Ok(());
            }
            Ok(Err(error)) => format!("向 {label} 发送失败：{error}"),
            Err(_) => format!("向 {label} 发送超时"),
        };

        self.stop_timer();
        if let Route::Stream(id) = route {
            self.remove_peer(*id).await;
        }
        Err(error)
    }

    async fn tick(&mut self) {
        let Some(mut timer) = self.timer.take() else {
            return;
        };
        match self.write(&timer.route, &timer.packet).await {
            Ok(()) => {
                let scheduled = timer.next + timer.interval;
                let now = Instant::now();
                timer.next = if scheduled > now {
                    scheduled
                } else {
                    now + timer.interval
                };
                self.timer = Some(timer);
            }
            Err(message) => {
                (self.emit)(Event::Timer { running: false });
                (self.emit)(Event::Fault { message });
            }
        }
    }

    fn stop_timer(&mut self) {
        if self.timer.take().is_some() {
            (self.emit)(Event::Timer { running: false });
        }
    }

    async fn remove_peer(&mut self, id: u64) {
        let Some(peer) = self.peers.remove(&id) else {
            return;
        };
        peer.close().await;
        if self
            .timer
            .as_ref()
            .is_some_and(|timer| matches!(timer.route, Route::Stream(peer) if peer == id))
        {
            self.stop_timer();
        }
        if self.server {
            self.status();
        } else {
            self.alive = false;
        }
    }

    async fn receive(&mut self, message: Incoming) {
        match message {
            Incoming::Accepted(stream, address) => {
                if self.peers.len() >= MAX_PEERS {
                    (self.emit)(Event::Fault {
                        message: "已达到 TCP 客户端连接上限".into(),
                    });
                    return;
                }
                let id = self.next_peer;
                self.next_peer += 1;
                let _ = stream.set_nodelay(true);
                self.add_stream(id, address.to_string(), stream);
                self.status();
            }
            Incoming::Bytes { id, bytes, source } => {
                let (packets, overflow, label) = if let Some(source) = source {
                    let (packets, overflow) = self
                        .framing
                        .as_ref()
                        .map(|framing| Decoder::new(framing).feed(&bytes))
                        .unwrap_or_else(|| (vec![raw_packet(bytes)], false));
                    (packets, overflow, source.to_string())
                } else if let Some(peer) = self.peers.get_mut(&id) {
                    let (packets, overflow) = peer
                        .decoder
                        .as_mut()
                        .map(|decoder| decoder.feed(&bytes))
                        .unwrap_or_else(|| (vec![raw_packet(bytes)], false));
                    (packets, overflow, peer.label.clone())
                } else {
                    return;
                };
                if !packets.is_empty() {
                    let rows = packets
                        .into_iter()
                        .map(|packet| Record::new(Direction::Rx, Some(label.clone()), packet))
                        .collect();
                    (self.emit)(Event::Messages { rows });
                }
                if overflow {
                    (self.emit)(Event::Fault {
                        message: format!("{label} 的未完成帧超过 1 MiB，已重新寻找帧头"),
                    });
                }
            }
            Incoming::Ended { id, error } => {
                if self.peers.contains_key(&id) {
                    if let Some(message) = error {
                        (self.emit)(Event::Fault { message });
                    }
                    self.remove_peer(id).await;
                }
            }
            Incoming::SocketError(message) => {
                (self.emit)(Event::Fault { message });
                self.alive = false;
            }
        }
    }
}

#[cfg(windows)]
fn apply_extended_parity(
    stream: &tokio_serial::SerialStream,
    parity: &Parity,
) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Devices::Communication::{
        DCB, GetCommState, MARKPARITY, SPACEPARITY, SetCommState,
    };

    let parity = match parity {
        Parity::Mark => MARKPARITY,
        Parity::Space => SPACEPARITY,
        _ => return Ok(()),
    };
    let handle = stream.as_raw_handle();
    let mut dcb = DCB {
        DCBlength: size_of::<DCB>() as u32,
        ..Default::default()
    };
    // 串口持有有效的 Windows 句柄，DCB 在调用期间可写。
    if unsafe { GetCommState(handle, &mut dcb) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    // 保留驱动返回的波特率、流控等设置，仅更新校验类型和 fParity。
    dcb.Parity = parity;
    dcb._bitfield |= 1 << 1;
    if unsafe { SetCommState(handle, &dcb) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
fn apply_extended_parity(
    _stream: &tokio_serial::SerialStream,
    parity: &Parity,
) -> std::io::Result<()> {
    match parity {
        Parity::Mark | Parity::Space => Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Mark 和 Space 校验需要 Windows 串口",
        )),
        _ => Ok(()),
    }
}

fn host_tuple(address: &Address) -> Result<(&str, u16), String> {
    let host = address.host.trim();
    if host.is_empty() {
        return Err("请填写地址".into());
    }
    Ok((host, address.port))
}

async fn wait_for_tick(next: Option<Instant>) {
    match next {
        Some(next) => sleep_until(next).await,
        None => pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loopback() -> Address {
        Address {
            host: "127.0.0.1".into(),
            port: 0,
        }
    }

    fn framing() -> Framing {
        Framing {
            header: vec![0xaa, 0x55],
            tail: vec![0x0d, 0x0a],
        }
    }

    async fn next<T>(
        events: &mut mpsc::UnboundedReceiver<Event>,
        select: impl Fn(Event) -> Option<T>,
    ) -> T {
        timeout(Duration::from_secs(3), async {
            while let Some(event) = events.recv().await {
                if let Some(value) = select(event) {
                    return value;
                }
            }
            panic!("事件通道提前关闭");
        })
        .await
        .expect("未收到预期的收发事件")
    }

    async fn endpoint(events: &mut mpsc::UnboundedReceiver<Event>) -> SocketAddr {
        next(events, |event| match event {
            Event::Status {
                connected: true,
                endpoint,
                ..
            } => Some(endpoint.parse().unwrap()),
            _ => None,
        })
        .await
    }

    async fn message(events: &mut mpsc::UnboundedReceiver<Event>) -> Record {
        next(events, |event| match event {
            Event::Messages { mut rows } if rows.len() == 1 => rows.pop(),
            Event::Messages { rows } => panic!("预期一条日志，得到 {} 条", rows.len()),
            _ => None,
        })
        .await
    }

    #[tokio::test]
    async fn local_listeners_reject_occupied_ports() {
        let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let udp_port = udp.local_addr().unwrap().port();
        let udp_options = || Connection::Udp {
            mode: UdpMode::Unicast,
            group: None,
            interface: None,
            multicast_ttl: None,
            local: Address {
                host: "127.0.0.1".into(),
                port: udp_port,
            },
        };
        let udp_error = connect(udp_options(), None, |_| {})
            .await
            .err()
            .expect("应拒绝绑定已被占用的 UDP 端口");
        assert!(udp_error.starts_with("无法绑定 UDP"));
        drop(udp);
        connect(udp_options(), None, |_| {})
            .await
            .unwrap()
            .close()
            .await;

        let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let tcp_port = tcp.local_addr().unwrap().port();
        let tcp_options = || Connection::TcpServer {
            local: Address {
                host: "127.0.0.1".into(),
                port: tcp_port,
            },
        };
        let tcp_error = connect(tcp_options(), None, |_| {})
            .await
            .err()
            .expect("应拒绝绑定已被占用的 TCP 监听端口");
        assert!(tcp_error.starts_with("TCP 监听失败"));
        drop(tcp);
        connect(tcp_options(), None, |_| {})
            .await
            .unwrap()
            .close()
            .await;
    }

    #[tokio::test]
    async fn udp_multicast_rejects_invalid_group_and_releases_port() {
        let probe = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);
        let result = connect(
            Connection::Udp {
                local: Address {
                    host: "127.0.0.1".into(),
                    port,
                },
                mode: UdpMode::Multicast,
                group: Some("127.0.0.1".into()),
                interface: Some("127.0.0.1".into()),
                multicast_ttl: None,
            },
            None,
            |_| {},
        )
        .await;
        assert!(result.err().unwrap().contains("组播地址"));
        UdpSocket::bind(("127.0.0.1", port)).await.unwrap();
    }

    #[tokio::test]
    async fn udp_multicast_loopback_sends_and_receives() {
        let (tx, mut events) = mpsc::unbounded_channel();
        let session = connect(
            Connection::Udp {
                local: Address {
                    host: "127.0.0.1".into(),
                    port: 0,
                },
                mode: UdpMode::Multicast,
                group: Some("239.255.23.42".into()),
                interface: None,
                multicast_ttl: Some(255),
            },
            None,
            move |event| {
                let _ = tx.send(event);
            },
        )
        .await
        .unwrap();
        let local = endpoint(&mut events).await;
        session
            .send(
                raw_packet(b"multicast".to_vec()),
                Some(Target::Udp {
                    host: "239.255.23.42".into(),
                    port: local.port(),
                }),
            )
            .await
            .unwrap();
        let tx_record = message(&mut events).await;
        let rx_record = message(&mut events).await;
        assert!(matches!(tx_record.direction, Direction::Tx));
        assert!(matches!(rx_record.direction, Direction::Rx));
        assert_eq!(rx_record.packet.bytes, b"multicast");
        session.close().await;
    }

    #[tokio::test]
    async fn udp_broadcast_loopback_sends_and_receives() {
        let remote = UdpSocket::bind("0.0.0.0:0").await.unwrap();
        let port = remote.local_addr().unwrap().port();
        let (tx, mut events) = mpsc::unbounded_channel();
        let session = connect(
            Connection::Udp {
                local: loopback(),
                mode: UdpMode::Broadcast,
                group: None,
                interface: None,
                multicast_ttl: None,
            },
            None,
            move |event| {
                let _ = tx.send(event);
            },
        )
        .await
        .unwrap();
        endpoint(&mut events).await;
        session
            .send(
                raw_packet(b"broadcast".to_vec()),
                Some(Target::Udp {
                    host: "127.255.255.255".into(),
                    port,
                }),
            )
            .await
            .unwrap();
        let mut buffer = [0; 32];
        let (length, _) = timeout(Duration::from_secs(3), remote.recv_from(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&buffer[..length], b"broadcast");
        session.close().await;
    }

    #[tokio::test]
    async fn udp_loopback_preserves_datagram_boundaries() {
        let (tx, mut events) = mpsc::unbounded_channel();
        let session = connect(
            Connection::Udp {
                local: loopback(),
                mode: UdpMode::Unicast,
                group: None,
                interface: None,
                multicast_ttl: None,
            },
            Some(framing()),
            move |event| {
                let _ = tx.send(event);
            },
        )
        .await
        .unwrap();
        let address = endpoint(&mut events).await;
        let remote = UdpSocket::bind("127.0.0.1:0").await.unwrap();

        remote.send_to(&[0xaa, 0x55, 1], address).await.unwrap();
        remote.send_to(&[0x0d, 0x0a], address).await.unwrap();
        remote
            .send_to(&[0xaa, 0x55, 2, 0x0d, 0x0a], address)
            .await
            .unwrap();
        let received = message(&mut events).await;
        assert!(matches!(received.direction, Direction::Rx));
        assert_eq!(received.packet.bytes, [0xaa, 0x55, 2, 0x0d, 0x0a]);
        assert_eq!(received.packet.header_len, 2);
        assert_eq!(received.packet.tail_len, 2);

        session.set_framing(None).await.unwrap();
        remote.send_to(b"raw", address).await.unwrap();
        let raw = message(&mut events).await;
        assert_eq!(raw.packet.bytes, b"raw");
        assert_eq!(raw.packet.header_len, 0);

        let target = remote.local_addr().unwrap();
        session
            .send(
                raw_packet(b"out".to_vec()),
                Some(Target::Udp {
                    host: target.ip().to_string(),
                    port: target.port(),
                }),
            )
            .await
            .unwrap();
        let mut buffer = [0; 16];
        let (length, source) = timeout(Duration::from_secs(3), remote.recv_from(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&buffer[..length], b"out");
        assert_eq!(source, address);
        assert!(matches!(
            message(&mut events).await.direction,
            Direction::Tx
        ));
        session.close().await;

        // 关闭完成后，同一绑定地址可以立即复用。
        UdpSocket::bind(address).await.unwrap();
    }

    #[tokio::test]
    async fn tcp_server_isolates_peers_and_stops_timer_when_target_leaves() {
        let (tx, mut events) = mpsc::unbounded_channel();
        let session = connect(
            Connection::TcpServer { local: loopback() },
            Some(framing()),
            move |event| {
                let _ = tx.send(event);
            },
        )
        .await
        .unwrap();
        let address = endpoint(&mut events).await;
        let mut first = TcpStream::connect(address).await.unwrap();
        let mut second = TcpStream::connect(address).await.unwrap();
        let clients = next(&mut events, |event| match event {
            Event::Status { clients, .. } if clients.len() == 2 => Some(clients),
            _ => None,
        })
        .await;
        let first_label = first.local_addr().unwrap().to_string();
        let second_label = second.local_addr().unwrap().to_string();
        let first_id = clients
            .iter()
            .find(|peer| peer.address == first_label)
            .unwrap()
            .id;
        let second_id = clients
            .iter()
            .find(|peer| peer.address == second_label)
            .unwrap()
            .id;

        first.write_all(&[0xaa, 0x55, 1, 0x0d]).await.unwrap();
        second
            .write_all(&[0x0a, 0xaa, 0x55, 2, 0x0d, 0x0a])
            .await
            .unwrap();
        let second_frame = message(&mut events).await;
        assert_eq!(second_frame.peer.as_deref(), Some(second_label.as_str()));
        assert_eq!(second_frame.packet.bytes, [0xaa, 0x55, 2, 0x0d, 0x0a]);

        first.write_all(&[0x0a]).await.unwrap();
        let first_frame = message(&mut events).await;
        assert_eq!(first_frame.peer.as_deref(), Some(first_label.as_str()));
        assert_eq!(first_frame.packet.bytes, [0xaa, 0x55, 1, 0x0d, 0x0a]);

        session
            .start_timer(
                raw_packet(b"one".to_vec()),
                Some(Target::Peer { id: first_id }),
                1000,
            )
            .await
            .unwrap();
        next(&mut events, |event| {
            matches!(event, Event::Timer { running: true }).then_some(())
        })
        .await;
        let mut buffer = [0; 3];
        timeout(Duration::from_secs(3), first.read_exact(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&buffer, b"one");

        drop(first);
        next(&mut events, |event| {
            matches!(event, Event::Timer { running: false }).then_some(())
        })
        .await;
        session
            .send(
                raw_packet(b"two".to_vec()),
                Some(Target::Peer { id: second_id }),
            )
            .await
            .unwrap();
        timeout(Duration::from_secs(3), second.read_exact(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&buffer, b"two");
        session.close().await;
    }

    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "需要本机可用的 Windows 虚拟串口"]
    async fn virtual_serial_port_applies_mark_and_space_parity() {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Devices::Communication::{
            DCB, GetCommState, MARKPARITY, SPACEPARITY,
        };

        let port = std::env::var("PORT_ASSISTANT_TEST_PORT").expect("请指定虚拟串口");
        let stream = tokio_serial::new(&port, 9600).open_native_async().unwrap();

        for (mode, expected) in [(Parity::Mark, MARKPARITY), (Parity::Space, SPACEPARITY)] {
            apply_extended_parity(&stream, &mode).unwrap();
            let mut dcb = DCB {
                DCBlength: size_of::<DCB>() as u32,
                ..Default::default()
            };
            // 测试持有已打开的串口，直接回读驱动报告的配置。
            assert_ne!(unsafe { GetCommState(stream.as_raw_handle(), &mut dcb) }, 0);
            assert_eq!(dcb.Parity, expected);
            assert_eq!(dcb.BaudRate, 9600);
        }
    }

    #[tokio::test]
    async fn tcp_client_writes_and_receives() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, mut events) = mpsc::unbounded_channel();
        let session = connect(
            Connection::TcpClient {
                remote: Address {
                    host: "127.0.0.1".into(),
                    port,
                },
            },
            None,
            move |event| {
                let _ = tx.send(event);
            },
        )
        .await
        .unwrap();
        let (mut peer, _) = timeout(Duration::from_secs(3), listener.accept())
            .await
            .unwrap()
            .unwrap();
        peer.write_all(&[42]).await.unwrap();
        assert_eq!(message(&mut events).await.packet.bytes, [42]);
        session.send(raw_packet(vec![43]), None).await.unwrap();
        assert_eq!(
            timeout(Duration::from_secs(3), peer.read_u8())
                .await
                .unwrap()
                .unwrap(),
            43
        );
        session.close().await;
    }
}
