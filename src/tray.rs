use crate::{
    clipboard::ClipboardCommand,
    history::HistoryItem,
    net::AppContext,
    pairing::PendingInfo,
    protocol::PROTOCOL_VERSION,
};
use anyhow::{Context, Result};
use std::{
    collections::{HashMap, HashSet},
    sync::{atomic::Ordering, Arc},
};
use std::time::{Duration, Instant};
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

#[cfg(target_os = "macos")]
const ICON_OFF: &[u8] = include_bytes!("../assets/tray-mac-off.png");
#[cfg(target_os = "macos")]
const ICON_ON: &[u8] = include_bytes!("../assets/tray-mac-on.png");
#[cfg(target_os = "macos")]
const ICON_ACTIVE: &[u8] = include_bytes!("../assets/tray-mac-active.png");

#[cfg(not(target_os = "macos"))]
const ICON_OFF: &[u8] = include_bytes!("../assets/tray-off-color.png");
#[cfg(not(target_os = "macos"))]
const ICON_ON: &[u8] = include_bytes!("../assets/tray-on-color.png");
#[cfg(not(target_os = "macos"))]
const ICON_ACTIVE: &[u8] = include_bytes!("../assets/tray-active-color.png");

#[derive(Debug)]
enum UserEvent {
    Menu(MenuId),
    OpenConfig(&'static str),
    Peers(Vec<(String, String)>),
    Discovered(Vec<DiscoveredEntry>),
    History(Vec<Arc<HistoryItem>>),
    Pairing(Vec<PendingInfo>),
    Activity,
}

#[derive(Debug, Clone)]
struct DiscoveredEntry {
    device_id: String,
    name: String,
    addr: String,
    protocol: u32,
    online: bool,
}

struct TrayMenu {
    sync: CheckMenuItem,
    autostart: CheckMenuItem,
    scan: MenuItem,
    discovered: Submenu,
    discovered_items: Vec<MenuItem>,
    discovered_actions: HashMap<String, String>,
    requests: Submenu,
    request_items: Vec<MenuItem>,
    request_actions: HashMap<String, (String, bool)>,
    devices: Submenu,
    device_items: Vec<MenuItem>,
    history: Submenu,
    history_items: Vec<MenuItem>,
    history_actions: HashMap<String, Arc<HistoryItem>>,
    clear_history: MenuItem,
    connection: MenuItem,
    local: MenuItem,
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
        let scan = MenuItem::new("扫描局域网设备", true, None);
        let discovered = Submenu::new("发现的设备 (0)", true);
        let discovered_placeholder = MenuItem::new("未发现设备", false, None);
        discovered.append(&discovered_placeholder)?;
        let requests = Submenu::new("连接请求 (0)", true);
        let request_placeholder = MenuItem::new("无待处理请求", false, None);
        requests.append(&request_placeholder)?;
        let devices = Submenu::new("在线设备 (0)", true);
        let placeholder = MenuItem::new("无在线设备", false, None);
        devices.append(&placeholder)?;
        let history = Submenu::new("剪贴历史 (0)", true);
        let history_placeholder = MenuItem::new("暂无剪贴历史", false, None);
        history.append(&history_placeholder)?;
        let clear_history = MenuItem::new("清空剪贴历史", false, None);
        let connection = MenuItem::new("连接设置", true, None);
        let local = MenuItem::new("本地设置", true, None);
        let test_sound = MenuItem::new("测试提示音", true, None);
        let quit = MenuItem::new("退出 LanPaster", true, None);

        // Group the menu by purpose: sync at the top, then network devices,
        // clipboard history, settings, and finally app-level actions.
        menu.append(&sync)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&scan)?;
        menu.append(&discovered)?;
        menu.append(&requests)?;
        menu.append(&devices)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&history)?;
        menu.append(&clear_history)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&connection)?;
        menu.append(&local)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&autostart)?;
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
                scan,
                discovered,
                discovered_items: vec![discovered_placeholder],
                discovered_actions: HashMap::new(),
                requests,
                request_items: vec![request_placeholder],
                request_actions: HashMap::new(),
                devices,
                device_items: vec![placeholder],
                history,
                history_items: vec![history_placeholder],
                history_actions: HashMap::new(),
                clear_history,
                connection,
                local,
                test_sound,
                quit,
                online_count: 0,
            },
        ))
    }

    fn update_requests(&mut self, pending: &[PendingInfo]) {
        for item in self.request_items.drain(..) {
            let _ = self.requests.remove(&item);
        }
        self.request_actions.clear();
        if pending.is_empty() {
            let item = MenuItem::new("无待处理请求", false, None);
            let _ = self.requests.append(&item);
            self.request_items.push(item);
        } else {
            for request in pending {
                let approve = MenuItem::new(
                    format!("同意：{} ({})", request.name, request.fingerprint),
                    true,
                    None,
                );
                self.request_actions
                    .insert(approve.id().as_ref().to_string(), (request.key.clone(), true));
                let _ = self.requests.append(&approve);
                self.request_items.push(approve);

                let deny = MenuItem::new(format!("拒绝：{}", request.name), true, None);
                self.request_actions
                    .insert(deny.id().as_ref().to_string(), (request.key.clone(), false));
                let _ = self.requests.append(&deny);
                self.request_items.push(deny);
            }
        }
        self.requests.set_text(format!("连接请求 ({})", pending.len()));
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

    fn update_discovered(&mut self, peers: &[DiscoveredEntry], mdns_enabled: bool) {
        self.scan.set_enabled(mdns_enabled);
        self.scan.set_text(if mdns_enabled {
            "扫描局域网设备"
        } else {
            "扫描局域网设备（mDNS 已关闭）"
        });
        for item in self.discovered_items.drain(..) {
            let _ = self.discovered.remove(&item);
        }
        self.discovered_actions.clear();
        if peers.is_empty() {
            let text = if mdns_enabled {
                "未发现设备，点击上方扫描"
            } else {
                "mDNS 已在设置中关闭"
            };
            let item = MenuItem::new(text, false, None);
            let _ = self.discovered.append(&item);
            self.discovered_items.push(item);
        } else {
            for peer in peers {
                let compatible = peer.protocol == PROTOCOL_VERSION;
                let label = if peer.online {
                    format!("已连接：{}", peer.name)
                } else if !compatible {
                    let version = if peer.protocol == 0 {
                        "未知".to_string()
                    } else {
                        format!("v{}", peer.protocol)
                    };
                    format!(
                        "{}（{}，版本 {version} 不兼容）",
                        peer.name, peer.addr
                    )
                } else {
                    format!("连接：{}（{}）", peer.name, peer.addr)
                };
                let enabled = !peer.online && compatible;
                let item = MenuItem::new(label, enabled, None);
                if enabled {
                    self.discovered_actions
                        .insert(item.id().as_ref().to_string(), peer.device_id.clone());
                }
                let _ = self.discovered.append(&item);
                self.discovered_items.push(item);
            }
        }
        self.discovered
            .set_text(format!("发现的设备 ({})", peers.len()));
    }

    fn update_history(&mut self, items: &[Arc<HistoryItem>]) {
        for item in self.history_items.drain(..) {
            let _ = self.history.remove(&item);
        }
        self.history_actions.clear();
        if items.is_empty() {
            let item = MenuItem::new("暂无剪贴历史", false, None);
            let _ = self.history.append(&item);
            self.history_items.push(item);
        } else {
            for entry in items {
                let restorable = entry.is_restorable();
                let origin: String = entry.origin.chars().take(20).collect();
                let label = if restorable {
                    format!("[{origin}] {}", entry.preview())
                } else {
                    format!("[{origin}] {}（文件已失效）", entry.preview())
                };
                let item = MenuItem::new(label, restorable, None);
                if restorable {
                    self.history_actions
                        .insert(item.id().as_ref().to_string(), entry.clone());
                }
                let _ = self.history.append(&item);
                self.history_items.push(item);
            }
        }
        self.history.set_text(format!("剪贴历史 ({})", items.len()));
        self.clear_history.set_enabled(!items.is_empty());
    }
}

struct AppHandler {
    app: Arc<AppContext>,
    runtime: tokio::runtime::Handle,
    menu: Option<TrayMenu>,
    tray: Option<TrayIcon>,
    failed: bool,
    active_until: Option<Instant>,
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
                if id == *menu.scan.id() {
                    self.app.request_scan();
                    return;
                }
                if let Some(device_id) = menu.discovered_actions.get(id.as_ref()).cloned() {
                    let app = self.app.clone();
                    self.runtime.spawn(async move {
                        app.connect_discovered(&device_id).await;
                    });
                    return;
                }
                if let Some(entry) = menu.history_actions.get(id.as_ref()).cloned() {
                    let clipboard = self.app.clipboard_cmd.clone();
                    let origin = entry.origin.clone();
                    self.runtime.spawn(async move {
                        let Some(payload) = entry.to_remote() else {
                            warn!("clipboard history entry is no longer available");
                            return;
                        };
                        let (respond, response) = tokio::sync::oneshot::channel();
                        if clipboard
                            .send(ClipboardCommand::Write {
                                payload,
                                origin: origin.clone(),
                                record: false,
                                respond,
                            })
                            .is_ok()
                        {
                            match response.await {
                                Ok(Ok(())) => {
                                    info!("restored clipboard history from {origin}")
                                }
                                Ok(Err(err)) => {
                                    warn!("restore clipboard history failed: {err:#}")
                                }
                                Err(_) => warn!("clipboard thread unavailable"),
                            }
                        }
                    });
                    return;
                }
                if id == *menu.clear_history.id() {
                    self.app.history.clear();
                    menu.update_history(&[]);
                    return;
                }
                if let Some((key, approve)) = menu.request_actions.get(id.as_ref()).cloned() {
                    if approve {
                        let _ = self.app.pairing.approve(&key);
                    } else {
                        let _ = self.app.pairing.deny(&key);
                    }
                    menu.update_requests(&self.app.pairing.pending_list());
                    return;
                }
                if id == *menu.sync.id() {
                    let enabled = !self.app.sync_enabled.load(Ordering::Relaxed);
                    self.app.sync_enabled.store(enabled, Ordering::Relaxed);
                    menu.sync.set_checked(enabled);
                    if enabled {
                        let _ = self.app.clipboard_cmd.send(ClipboardCommand::Push);
                    }
                    update_tray_status(
                        menu,
                        self.tray.as_ref(),
                        &self.app,
                        self.active_until.is_some(),
                    );
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
                } else if id == *menu.test_sound.id() {
                    crate::sound::play();
                } else if id == *menu.quit.id() {
                    info!("quit requested from tray");
                    event_loop.exit();
                }
            }
            UserEvent::OpenConfig(panel) => open_config_window(panel),
            UserEvent::Pairing(pending) => {
                if let Some(menu) = self.menu.as_mut() {
                    menu.update_requests(&pending);
                }
            }
            UserEvent::Peers(peers) => {
                debug!("tray device update: {} peer(s)", peers.len());
                if let Some(menu) = self.menu.as_mut() {
                    menu.update_devices(&peers);
                    update_tray_status(
                        menu,
                        self.tray.as_ref(),
                        &self.app,
                        self.active_until.is_some(),
                    );
                }
            }
            UserEvent::Discovered(peers) => {
                debug!("tray discovery update: {} device(s)", peers.len());
                if let Some(menu) = self.menu.as_mut() {
                    menu.update_discovered(&peers, self.app.config.load().mdns_enabled);
                }
            }
            UserEvent::History(items) => {
                debug!("tray history update: {} item(s)", items.len());
                if let Some(menu) = self.menu.as_mut() {
                    menu.update_history(&items);
                }
            }
            UserEvent::Activity => {
                self.active_until = Some(Instant::now() + Duration::from_millis(1200));
                if let Some(menu) = self.menu.as_mut() {
                    update_tray_status(menu, self.tray.as_ref(), &self.app, true);
                }
                event_loop.set_control_flow(ControlFlow::WaitUntil(
                    self.active_until.unwrap(),
                ));
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(deadline) = self.active_until {
            if Instant::now() >= deadline {
                self.active_until = None;
                if let Some(menu) = self.menu.as_mut() {
                    update_tray_status(menu, self.tray.as_ref(), &self.app, false);
                }
                event_loop.set_control_flow(ControlFlow::Wait);
            } else {
                event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
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

    // mDNS discovery list refresh. The list itself lives in AppContext; this
    // task only mirrors it into the tray menu.
    let proxy = event_loop.create_proxy();
    let discovery_app = app.clone();
    runtime.spawn(async move {
        let mut last_key = String::new();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let discovered = discovery_app.discovered_list();
            let online: HashSet<String> = discovery_app
                .peers
                .snapshot()
                .await
                .into_iter()
                .map(|p| p.device_id.clone())
                .collect();
            let entries: Vec<DiscoveredEntry> = discovered
                .into_iter()
                .map(|peer| {
                    let online = online.contains(&peer.device_id);
                    DiscoveredEntry {
                        device_id: peer.device_id,
                        name: peer.device_name,
                        addr: peer.addr,
                        protocol: peer.protocol,
                        online,
                    }
                })
                .collect();
            let key = entries
                .iter()
                .map(|p| {
                    format!(
                        "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
                        p.device_id, p.name, p.addr, p.protocol, p.online
                    )
                })
                .collect::<Vec<_>>()
                .join("\u{1e}");
            if key == last_key {
                continue;
            }
            last_key = key;
            let _ = proxy.send_event(UserEvent::Discovered(entries));
        }
    });

    let proxy = event_loop.create_proxy();
    let history_app = app.clone();
    runtime.spawn(async move {
        let mut last_key = String::new();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let cfg = history_app.config.load_full();
            let items = history_app
                .history
                .snapshot(cfg.history_minutes, cfg.history_limit);
            let key = items
                .iter()
                .map(|item| format!("{}\u{1f}{}", item.id, item.is_restorable()))
                .collect::<Vec<_>>()
                .join("\u{1e}");
            if key == last_key {
                continue;
            }
            last_key = key;
            let _ = proxy.send_event(UserEvent::History(items));
        }
    });

    let proxy = event_loop.create_proxy();
    let activity = app.activity_seq.clone();
    runtime.spawn(async move {
        let mut last = activity.load(Ordering::Relaxed);
        loop {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let current = activity.load(Ordering::Relaxed);
            if current != last {
                last = current;
                let _ = proxy.send_event(UserEvent::Activity);
            }
        }
    });

    let proxy = event_loop.create_proxy();
    let pairing = app.pairing.clone();
    runtime.spawn(async move {
        let mut last_key = String::new();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let pending = pairing.pending_list();
            let key = pending
                .iter()
                .map(|p| format!("{}\u{1f}{}\u{1f}{}", p.key, p.name, p.fingerprint))
                .collect::<Vec<_>>()
                .join("\u{1e}");
            if key == last_key {
                continue;
            }
            last_key = key;
            let _ = proxy.send_event(UserEvent::Pairing(pending));
        }
    });

    let mut handler = AppHandler {
        app,
        runtime: runtime.clone(),
        menu: None,
        tray: None,
        failed: false,
        active_until: None,
    };
    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop.run_app(&mut handler)?;
    if handler.failed {
        anyhow::bail!("tray icon unavailable");
    }
    Ok(())
}

fn build_tray(app: &Arc<AppContext>) -> Result<(TrayMenu, TrayIcon)> {
    let (menu, mut menu_state) = TrayMenu::new()?;
    menu_state.update_discovered(&[], app.config.load().mdns_enabled);
    let icon = load_icon(ICON_OFF)?;
    let builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip(format!("LanPaster · {}", app.config.load().device_name));
    #[cfg(target_os = "macos")]
    let builder = builder.with_icon_templated(icon);
    #[cfg(not(target_os = "macos"))]
    let builder = builder.with_icon(icon);
    let tray = builder.build()?;
    Ok((menu_state, tray))
}

fn update_tray_status(
    menu: &TrayMenu,
    tray: Option<&TrayIcon>,
    app: &Arc<AppContext>,
    active: bool,
) {
    let enabled = app.sync_enabled.load(Ordering::Relaxed);
    if let Some(tray) = tray {
        let _ = tray.set_tooltip(Some(format!(
            "LanPaster · {} · {} 台在线 · {}{}",
            app.config.load().device_name,
            menu.online_count,
            if enabled { "同步开" } else { "同步关" },
            if active { " · 收到剪贴" } else { "" }
        )));
        let icon = if active {
            ICON_ACTIVE
        } else if !enabled || menu.online_count == 0 {
            ICON_OFF
        } else {
            ICON_ON
        };
        if let Ok(icon) = load_icon(icon) {
            #[cfg(target_os = "macos")]
            {
                let _ = tray.set_icon_templated(Some(icon));
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = tray.set_icon(Some(icon));
            }
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
                .env("LANPASTER_TRAY_UI", "1")
                .spawn()
            {
                warn!("failed to open config window: {err}");
            }
        }
        Err(err) => warn!("failed to locate current executable: {err}"),
    }
}
