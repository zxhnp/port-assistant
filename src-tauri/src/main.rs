#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use port_transport::{Connection, Event, Framing, Packet, Session, Target};
use tauri::ipc::Channel;
use tokio::sync::{MappedMutexGuard, Mutex, MutexGuard};

#[derive(Default)]
struct AppState(Mutex<Option<Session>>);

#[tauri::command]
fn open_new_instance() -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    std::process::Command::new(executable)
        .spawn()
        .map_err(|error| format!("无法打开新窗口：{error}"))?;
    Ok(())
}

impl AppState {
    async fn session(&self) -> Result<MappedMutexGuard<'_, Session>, String> {
        MutexGuard::try_map(self.0.lock().await, |slot| {
            slot.as_mut().filter(|session| !session.is_finished())
        })
        .map_err(|_| "当前没有活动连接".into())
    }
}

#[tauri::command]
async fn list_ports() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(port_transport::list_ports)
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn connect(
    state: tauri::State<'_, AppState>,
    connection: Connection,
    framing: Option<Framing>,
    events: Channel<Event>,
) -> Result<(), String> {
    let mut slot = state.0.lock().await;
    if let Some(session) = slot.take() {
        if !session.is_finished() {
            *slot = Some(session);
            return Err("请先关闭当前连接".into());
        }
        session.close().await;
    }
    *slot = Some(
        port_transport::connect(connection, framing, move |event| {
            let _ = events.send(event);
        })
        .await?,
    );
    Ok(())
}

#[tauri::command]
async fn disconnect(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let mut slot = state.0.lock().await;
    if let Some(session) = slot.take() {
        session.close().await;
    }
    Ok(())
}

#[tauri::command]
async fn set_framing(
    state: tauri::State<'_, AppState>,
    framing: Option<Framing>,
) -> Result<(), String> {
    state.session().await?.set_framing(framing).await
}

#[tauri::command]
async fn send_packet(
    state: tauri::State<'_, AppState>,
    packet: Packet,
    target: Option<Target>,
) -> Result<(), String> {
    state.session().await?.send(packet, target).await
}

#[tauri::command]
async fn start_timer(
    state: tauri::State<'_, AppState>,
    packet: Packet,
    target: Option<Target>,
    interval_ms: u32,
) -> Result<(), String> {
    state
        .session()
        .await?
        .start_timer(packet, target, interval_ms)
        .await
}

#[tauri::command]
async fn stop_timer(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.session().await?.stop_timer().await
}

fn main() {
    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            open_new_instance,
            list_ports,
            connect,
            disconnect,
            set_framing,
            send_packet,
            start_timer,
            stop_timer
        ])
        .run(tauri::generate_context!())
        .expect("端口助手启动失败");
}
