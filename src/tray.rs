use crate::{clipboard::ClipboardCommand, net::AppContext};
use anyhow::{Context, Result};
use std::sync::{
    atomic::Ordering,
    Arc,
};
use std::time::Duration;
use tray_icon::{
    menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu},
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
};
use tracing::{debug, info, warn};
use winit::{
    application::ApplicationHandler,
    event::StartCause,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
};

const ICON_OFF: &[u8] = include_bytes!("../assets/tray-off.png");
const ICON_ON: &[u8] = include_bytes!("../assets/tray-on.png");

#[derive(Debug)]
enum UserEvent {
    Menu(MenuId),
    OpenConfig(&'static str),
    Peers(Vec<(String, String)>),
}

struct TrayMenu {
    sync: CheckMenuItem,
    autostart: CheckMenuItem,
    devices: Submenu,
    device_items: Vec<MenuItem>,
    connection: MenuItem,
    local: MenuItem,
    push: MenuItem,
    test_sound: MenuItem,
    quit: MenuItem,
    online_count: usize,
}

impl TrayMenu {
    fn new() -> Result<(Menu, Self)> {
        let menu = Menu::new();
        let sync = CheckMenuItem::new("启用剪贴同步", true, true, None);
        let autostart = CheckMenuItem::new(
            "开机自启",
            true,
            crate::autostart::is_enabled(),
            None,
        );
        menu.append(&sync)?;
        menu.append(&autostart)?;
        menu.append(&PredefinedMenuItem::separator())?;

        let devices = Submenu::new("在线设备 (0)", true);
        let placeholder = MenuItem::new("无在线设备", false, None);
        devices.append(&placeholder)?;
        menu.append(&devices)?;

        let connection = MenuItem::new("连接设置", true, None);
        let local = MenuItem::new("本地设置", true, None);
        let push = MenuItem::new("推送当前剪贴板", true, None);
        let test_sound = MenuItem::new("测试提示音", true, None);
        let quit = MenuItem::new("退出 LanPaster", true, None);
        menu.append(&connection)?;
        menu.append(&local)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&push)?;
        menu.append(&test_sound)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&quit)?;

        // Keep the menu alive through the tray icon builder below; item handles
        // keep referring to the same native menu items after the menu is moved.
        Ok((
            menu,
            Self {
                sync,
                autostart,
                devices,
                device_items: vec![placeholder],
                connection,
                local,
                push,
                test_sound,
                quit,
                online_count: 0,
            },
        ))
    }

    fn update_devices(&mut self, peers: &[(String, String)]) {
        for item in self.device_items.drain(..) {
            let _ = self.devices.remove(&item);
        }
        if peers.is_empty() {
            let item = MenuItem::new("无在线设备", false, None);
            let _ = self.devices.append(&item);
            self.device_items.push(item);
        } else {
            for (name, id) in peers {
                let short = &id[..id.len().min(8)];
                let item = MenuItem::new(format!("{name} ({short})"), false, None);
                let _ = self.devices.append(&item);
                self.device_items.push(item);
            }
        }
        self.online_count = peers.len();
        self.devices.set_text(format!("在线设备 ({})", peers.len()));
    }
}

struct AppHandler {
    app: Arc<AppContext>,
    menu: Option<TrayMenu>,
    tray: Option<TrayIcon>,
    failed: bool,
}

impl ApplicationHandler<UserEvent> for AppHandler {
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {}

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        _event: winit::event::WindowEvent,
    ) {
    }

    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        if cause != StartCause::Init || self.tray.is_some() {
            return;
        }
        match build_tray(&self.app) {
            Ok((menu, tray)) => {
                self.menu = Some(menu);
                self.tray = Some(tray);
                info!("tray icon started");
            }
            Err(err) => {
                warn!("failed to create tray icon: {err:#}");
                self.failed = true;
                event_loop.exit();
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Menu(id) => {
                debug!("tray menu event: {id:?}");
                let Some(menu) = self.menu.as_mut() else {
                    return;
                };
                if id == *menu.sync.id() {
                    let enabled = !self.app.sync_enabled.load(Ordering::Relaxed);
                    self.app.sync_enabled.store(enabled, Ordering::Relaxed);
                    menu.sync.set_checked(enabled);
                    if enabled {
                        let _ = self.app.clipboard_cmd.send(ClipboardCommand::Push);
                    }
                    update_tray_status(menu, self.tray.as_ref(), &self.app);
                } else if id == *menu.autostart.id() {
                    let enabled = !crate::autostart::is_enabled();
                    match crate::autostart::set(enabled) {
                        Ok(()) => menu.autostart.set_checked(enabled),
                        Err(err) => {
                            warn!("failed to update autostart: {err:#}");
                            menu.autostart
                                .set_checked(crate::autostart::is_enabled());
                        }
                    }
                } else if id == *menu.connection.id() {
                    open_config_window("connection");
                } else if id == *menu.local.id() {
                    open_config_window("local");
                } else if id == *menu.push.id() {
                    if self.app.sync_enabled.load(Ordering::Relaxed) {
                        let _ = self.app.clipboard_cmd.send(ClipboardCommand::Push);
                    }
                } else if id == *menu.test_sound.id() {
                    crate::sound::play();
                } else if id == *menu.quit.id() {
                    info!("quit requested from tray");
                    event_loop.exit();
                }
            }
            UserEvent::OpenConfig(panel) => open_config_window(panel),
            UserEvent::Peers(peers) => {
                debug!("tray device update: {} peer(s)", peers.len());
                if let Some(menu) = self.menu.as_mut() {
                    menu.update_devices(&peers);
                    update_tray_status(menu, self.tray.as_ref(), &self.app);
                }
            }
        }
    }
}

pub fn run_tray(app: Arc<AppContext>, runtime: tokio::runtime::Handle) -> Result<()> {
    let mut builder = EventLoop::<UserEvent>::with_user_event();
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder.with_activation_policy(ActivationPolicy::Accessory);
    }
    let event_loop = builder.build().context("create winit event loop")?;

    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let _ = proxy.send_event(UserEvent::Menu(event.id().clone()));
    }));

    let proxy = event_loop.create_proxy();
    TrayIconEvent::set_event_handler(Some(move |event| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            let _ = proxy.send_event(UserEvent::OpenConfig("connection"));
        }
    }));

    // Device list refresh. The tray menu must be updated on the event-loop thread.
    let proxy = event_loop.create_proxy();
    let peer_app = app.clone();
    runtime.spawn(async move {
        let mut last_key = String::new();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let peers = peer_app.peers.snapshot().await;
            let mut list: Vec<(String, String)> = peers
                .iter()
                .map(|p| (p.device_name.clone(), p.device_id.clone()))
                .collect();
            list.sort();
            let device_name = peer_app.config.load().device_name.clone();
            let key = format!(
                "{device_name}\u{1d}{}",
                list.iter()
                    .map(|(n, id)| format!("{n}\u{1f}{id}"))
                    .collect::<Vec<_>>()
                    .join("\u{1e}")
            );
            if key == last_key {
                continue;
            }
            last_key = key;
            let _ = proxy.send_event(UserEvent::Peers(list));
        }
    });

    let mut handler = AppHandler {
        app,
        menu: None,
        tray: None,
        failed: false,
    };
    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop.run_app(&mut handler)?;
    if handler.failed {
        anyhow::bail!("tray icon unavailable");
    }
    Ok(())
}

fn build_tray(app: &Arc<AppContext>) -> Result<(TrayMenu, TrayIcon)> {
    let (menu, menu_state) = TrayMenu::new()?;
    let icon = load_icon(ICON_OFF)?;
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(icon)
        .with_tooltip(format!("LanPaster · {}", app.config.load().device_name))
        .build()?;
    Ok((menu_state, tray))
}

fn update_tray_status(menu: &TrayMenu, tray: Option<&TrayIcon>, app: &Arc<AppContext>) {
    let enabled = app.sync_enabled.load(Ordering::Relaxed);
    if let Some(tray) = tray {
        let _ = tray.set_tooltip(Some(format!(
            "LanPaster · {} · {} 台在线 · {}",
            app.config.load().device_name,
            menu.online_count,
            if enabled { "同步开" } else { "同步关" }
        )));
        let icon = if !enabled {
            ICON_OFF
        } else if menu.online_count > 0 {
            ICON_ON
        } else {
            ICON_OFF
        };
        if let Ok(icon) = load_icon(icon) {
            let _ = tray.set_icon(Some(icon));
        }
    }
}

fn load_icon(bytes: &[u8]) -> Result<Icon> {
    let image = image::load_from_memory(bytes)
        .context("decode tray icon")?
        .into_rgba8();
    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height).context("create tray icon")
}

fn open_config_window(panel: &str) {
    match std::env::current_exe() {
        Ok(exe) => {
            if let Err(err) = std::process::Command::new(exe)
                .arg("ui")
                .arg(panel)
                .spawn()
            {
                warn!("failed to open config window: {err}");
            }
        }
        Err(err) => warn!("failed to locate current executable: {err}"),
    }
}
