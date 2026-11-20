use crate::{
    clipboard::ClipboardCommand,
    history::HistoryItem,
    instance,
    net::AppContext,
    pairing::PendingInfo,
    protocol::PROTOCOL_VERSION,
};
use anyhow::{Context, Result};
use image::RgbaImage;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{atomic::Ordering, Arc},
};
use std::time::{Duration, Instant};
use tray_icon::{
    menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu},
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
};
#[cfg(target_os = "macos")]
use tray_icon::Rect;
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

/// 收到剪贴后高亮图标停留的时间。
const ACTIVITY_LINGER: Duration = Duration::from_millis(1400);
/// 高亮期间的呼吸动画时长；结束后保持高亮到 `ACTIVITY_LINGER`。
const ANIMATION_DURATION: Duration = Duration::from_millis(1000);
const FRAME_INTERVAL: Duration = Duration::from_millis(40);

#[derive(Debug)]
enum UserEvent {
    Menu(MenuId),
    OpenConfig(&'static str),
    Peers(Vec<(String, String)>),
    Discovered(Vec<DiscoveredEntry>),
    History(Vec<Arc<HistoryItem>>),
    Pairing(Vec<PendingInfo>),
    Activity,
    #[cfg(target_os = "macos")]
    ShowMenu(Rect),
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
    config_path: PathBuf,
    menu: Option<TrayMenu>,
    /// macOS 上菜单不再交给 NSStatusItem 自动弹出。系统会把菜单往
    /// 下放一个菜单栏的高度；这里保留原生 NSMenu，自己定位弹出。
    #[cfg(target_os = "macos")]
    native_menu: Option<Menu>,
    tray: Option<TrayIcon>,
    failed: bool,
    active_until: Option<Instant>,
    /// 高亮图标的原始 RGBA，用于按帧生成呼吸动画。
    active_image: Option<RgbaImage>,
    animation_start: Option<Instant>,
    animation_frame: Option<usize>,
}

impl AppHandler {
    /// 按时间推进收到剪贴后的呼吸动画，返回下一个需要处理的时刻。
    /// `None` 表示当前没有需要等待的状态，事件循环可以回到 `Wait`。
    fn advance_activity(&mut self, now: Instant) -> Option<Instant> {
        let until = self.active_until?;
        if now >= until {
            self.active_until = None;
            self.animation_start = None;
            self.animation_frame = None;
            if let Some(menu) = self.menu.as_ref() {
                update_tray_status(menu, self.tray.as_ref(), &self.app, false);
            }
            return None;
        }

        let Some(start) = self.animation_start else {
            return Some(until);
        };
        let elapsed = now.saturating_duration_since(start);
        if elapsed >= ANIMATION_DURATION {
            // 动画结束后保持完整亮度的高亮图标，直到 active_until 到期。
            if self.animation_frame.take().is_some() {
                self.apply_animation_frame(ANIMATION_DURATION);
            }
            return Some(until);
        }

        let frame = (elapsed.as_millis() / FRAME_INTERVAL.as_millis()) as usize;
        if self.animation_frame != Some(frame) {
            self.animation_frame = Some(frame);
            // 第 0 帧就是完整亮度，开始时已经由 update_tray_status 设置过。
            if frame > 0 {
                self.apply_animation_frame(elapsed);
            }
        }
        let next = start + FRAME_INTERVAL * (frame as u32 + 1);
        Some(next.min(until))
    }

    fn apply_animation_frame(&self, elapsed: Duration) {
        let (Some(tray), Some(image)) = (self.tray.as_ref(), self.active_image.as_ref()) else {
            return;
        };
        match dimmed_icon(image, activity_alpha(elapsed)) {
            Ok(icon) => set_tray_icon(tray, icon),
            Err(err) => warn!("failed to build tray animation frame: {err:#}"),
        }
    }
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
            Ok((menu, tray, _native_menu)) => {
                self.menu = Some(menu);
                #[cfg(target_os = "macos")]
                {
                    self.native_menu = _native_menu;
                }
                self.tray = Some(tray);
                match load_rgba(ICON_ACTIVE) {
                    Ok(image) => self.active_image = Some(image),
                    Err(err) => warn!("failed to decode active tray icon: {err:#}"),
                }
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
                    open_config_window(&self.config_path, "connection");
                } else if id == *menu.local.id() {
                    open_config_window(&self.config_path, "local");
                } else if id == *menu.test_sound.id() {
                    crate::sound::play();
                } else if id == *menu.quit.id() {
                    info!("quit requested from tray");
                    event_loop.exit();
                }
            }
            UserEvent::OpenConfig(panel) => open_config_window(&self.config_path, panel),
            #[cfg(target_os = "macos")]
            UserEvent::ShowMenu(rect) => {
                if let Some(menu) = self.native_menu.as_ref() {
                    show_native_menu(menu, rect);
                }
            }
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
                let now = Instant::now();
                self.active_until = Some(now + ACTIVITY_LINGER);
                self.animation_start = Some(now);
                self.animation_frame = None;
                if let Some(menu) = self.menu.as_ref() {
                    update_tray_status(menu, self.tray.as_ref(), &self.app, true);
                }
                match self.advance_activity(now) {
                    Some(deadline) => {
                        event_loop.set_control_flow(ControlFlow::WaitUntil(deadline))
                    }
                    None => event_loop.set_control_flow(ControlFlow::Wait),
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        match self.advance_activity(Instant::now()) {
            Some(deadline) => event_loop.set_control_flow(ControlFlow::WaitUntil(deadline)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }
}

pub fn run_tray(
    app: Arc<AppContext>,
    runtime: tokio::runtime::Handle,
    config_path: PathBuf,
) -> Result<()> {
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
            button,
            button_state: MouseButtonState::Up,
            rect: _click_rect,
            ..
        } = event
        {
            match button {
                MouseButton::Left => {
                    let _ = proxy.send_event(UserEvent::OpenConfig("connection"));
                }
                #[cfg(target_os = "macos")]
                MouseButton::Right => {
                    let _ = proxy.send_event(UserEvent::ShowMenu(_click_rect));
                }
                _ => {}
            }
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
    let activity = app.activity.clone();
    runtime.spawn(async move {
        loop {
            activity.notified().await;
            let _ = proxy.send_event(UserEvent::Activity);
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
        config_path,
        menu: None,
        #[cfg(target_os = "macos")]
        native_menu: None,
        tray: None,
        failed: false,
        active_until: None,
        active_image: None,
        animation_start: None,
        animation_frame: None,
    };
    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop.run_app(&mut handler)?;
    if handler.failed {
        anyhow::bail!("tray icon unavailable");
    }
    Ok(())
}

fn build_tray(app: &Arc<AppContext>) -> Result<(TrayMenu, TrayIcon, Option<Menu>)> {
    let (menu, mut menu_state) = TrayMenu::new()?;
    menu_state.update_discovered(&[], app.config.load().mdns_enabled);
    let icon = load_icon(ICON_OFF)?;
    let builder = TrayIconBuilder::new()
        .with_tooltip(format!("LanPaster · {}", app.config.load().device_name));
    // macOS：菜单自己持有并手动弹出，避开系统给状态栏菜单加的一个
    // 菜单栏高度的间隙。Windows / Linux 继续用 tray-icon 的原生菜单。
    #[cfg(not(target_os = "macos"))]
    let builder = builder.with_menu(Box::new(menu));
    #[cfg(target_os = "macos")]
    let builder = builder.with_icon_templated(icon);
    #[cfg(not(target_os = "macos"))]
    let builder = builder.with_icon(icon);
    let tray = builder.build()?;
    #[cfg(target_os = "macos")]
    let native_menu = Some(menu);
    #[cfg(not(target_os = "macos"))]
    let native_menu = None;
    Ok((menu_state, tray, native_menu))
}

/// 在 macOS 上手动弹出原生 NSMenu。菜单的键盘导航、子菜单、
/// 可访问性都还是 AppKit 提供的，只是位置改由我们指定。
#[cfg(target_os = "macos")]
fn show_native_menu(menu: &Menu, rect: Rect) {
    use objc2_app_kit::{NSMenu, NSScreen, NSEvent};
    use objc2_foundation::NSPoint;
    use tray_icon::menu::ContextMenu;

    let Some(mtm) = objc2::MainThreadMarker::new() else {
        return;
    };
    let Some(ns_menu) = (unsafe { (menu.ns_menu() as *const NSMenu).as_ref() }) else {
        return;
    };
    let mouse = NSEvent::mouseLocation();
    let screens = NSScreen::screens(mtm);
    let Some(screen) = screens.iter().find(|screen| {
        let frame = screen.frame();
        mouse.x >= frame.origin.x
            && mouse.x <= frame.origin.x + frame.size.width
            && mouse.y >= frame.origin.y
            && mouse.y <= frame.origin.y + frame.size.height
    }) else {
        return;
    };
    let visible = screen.visibleFrame();
    let scale = screen.backingScaleFactor();
    // rect 是物理像素；用点击所在屏幕的缩放换算成逻辑坐标。
    // 菜单左上角放在状态栏图标左边，并给菜单阴影留 6pt，
    // 避免第一项被菜单栏裁掉后出现滚动箭头。
    let left = if rect.size.width > 0 {
        rect.position.x / scale
    } else {
        mouse.x - 20.0
    };
    let location = NSPoint::new(left, visible.origin.y + visible.size.height - 6.0);
    ns_menu.popUpMenuPositioningItem_atLocation_inView(None, location, None);
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
            set_tray_icon(tray, icon);
        }
    }
}

fn set_tray_icon(tray: &TrayIcon, icon: Icon) {
    #[cfg(target_os = "macos")]
    {
        let _ = tray.set_icon_templated(Some(icon));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = tray.set_icon(Some(icon));
    }
}

fn load_rgba(bytes: &[u8]) -> Result<RgbaImage> {
    Ok(image::load_from_memory(bytes)
        .context("decode tray icon")?
        .into_rgba8())
}

fn load_icon(bytes: &[u8]) -> Result<Icon> {
    let image = load_rgba(bytes)?;
    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height).context("create tray icon")
}

/// 收到剪贴后的呼吸曲线：从 1.0 开始，两次平滑变暗后回到 1.0。
fn activity_alpha(elapsed: Duration) -> f32 {
    const MIN_ALPHA: f32 = 0.5;
    const BREATHS: f32 = 2.0;
    let phase = (elapsed.as_secs_f32() / ANIMATION_DURATION.as_secs_f32()).clamp(0.0, 1.0);
    let wave = 0.5 + 0.5 * (std::f32::consts::TAU * BREATHS * phase).cos();
    MIN_ALPHA + (1.0 - MIN_ALPHA) * wave
}

fn dimmed_icon(image: &RgbaImage, alpha: f32) -> Result<Icon> {
    let image = dim_image(image, alpha);
    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height).context("create tray icon")
}

fn dim_image(image: &RgbaImage, alpha: f32) -> RgbaImage {
    let alpha = alpha.clamp(0.0, 1.0);
    let mut out = image.clone();
    for pixel in out.pixels_mut() {
        pixel[3] = (f32::from(pixel[3]) * alpha).round() as u8;
    }
    out
}

fn open_config_window(config_path: &Path, panel: &str) {
    // 窗口是独立进程：已经开着就只请求它置前/切面板，避免每次点击都再
    // 启动一个 lanpaster ui 进程、多出一个窗口。
    if instance::ui_running(config_path) {
        if let Err(err) = instance::request_ui(config_path, panel) {
            warn!("failed to focus settings window: {err:#}");
        }
        return;
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn activity_alpha_breathes_twice_and_returns_to_full() {
        assert!((activity_alpha(Duration::ZERO) - 1.0).abs() < 1e-6);
        let quarter = ANIMATION_DURATION / 4;
        assert!(activity_alpha(quarter) < 0.55);
        assert!(activity_alpha(quarter * 2) > 0.999);
        assert!(activity_alpha(quarter * 3) < 0.55);
        assert!((activity_alpha(ANIMATION_DURATION) - 1.0).abs() < 1e-6);
        assert!((activity_alpha(ANIMATION_DURATION * 2) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn dim_image_scales_alpha_without_touching_color() {
        let image = RgbaImage::from_pixel(1, 1, Rgba([10, 20, 30, 200]));
        let dimmed = dim_image(&image, 0.5);
        assert_eq!(dimmed.get_pixel(0, 0), &Rgba([10, 20, 30, 100]));
        let full = dim_image(&image, 1.0);
        assert_eq!(full.get_pixel(0, 0), &Rgba([10, 20, 30, 200]));
    }
}
