use rodio::{source::SineWave, DeviceSinkBuilder, Player, Source};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};
use tracing::{debug, warn};

/// 音频设备失败后的冷却时间。期间收到远端剪贴只跳过提示音，不再反复打开设备。
const FAILURE_COOLDOWN: Duration = Duration::from_secs(5 * 60);

static COOLDOWN_UNTIL: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();

fn cooldown() -> &'static Mutex<Option<Instant>> {
    COOLDOWN_UNTIL.get_or_init(|| Mutex::new(None))
}

fn in_cooldown() -> bool {
    let mut guard = cooldown().lock().unwrap_or_else(|err| err.into_inner());
    match *guard {
        Some(until) if Instant::now() < until => true,
        Some(_) => {
            *guard = None;
            false
        }
        None => false,
    }
}

fn enter_cooldown() {
    let mut guard = cooldown().lock().unwrap_or_else(|err| err.into_inner());
    *guard = Some(Instant::now() + FAILURE_COOLDOWN);
}

fn clear_cooldown() {
    let mut guard = cooldown().lock().unwrap_or_else(|err| err.into_inner());
    *guard = None;
}

/// Play a short notification beep on a background thread.
pub fn play() {
    if in_cooldown() {
        debug!("notification sound skipped: audio device is in failure cooldown");
        return;
    }
    let _ = std::thread::Builder::new()
        .name("lanpaster-sound".to_string())
        .spawn(|| {
            if let Err(err) = play_blocking(false) {
                debug!("notification sound failed: {err:#}");
            }
        });
}

fn play_blocking(force: bool) -> anyhow::Result<()> {
    // 排队的播放线程可能在冷却开始后才真正执行，这里再检查一次；测试提示音用 force 绕过。
    if !force && in_cooldown() {
        return Ok(());
    }
    let result = play_stream();
    match &result {
        Ok(()) => clear_cooldown(),
        Err(_) => enter_cooldown(),
    }
    result
}

fn play_stream() -> anyhow::Result<()> {
    // 部分 Linux 输出设备（HDMI/DP、USB 声卡、被 PipeWire 接管的默认设备）在流启动后会
    // 持续返回 EIO，cpal 会把它当成可恢复错误热循环回调。这里只记录第一条，下面的等待
    // 逻辑发现失败后会立即拆流，避免日志刷屏和线程/设备句柄泄漏。
    let failed = Arc::new(AtomicBool::new(false));
    let error_flag = Arc::clone(&failed);
    let mut sink = DeviceSinkBuilder::from_default_device()
        .map_err(|e| anyhow::anyhow!("open audio device: {e}"))?
        .with_error_callback(move |err| {
            if !error_flag.swap(true, Ordering::Relaxed) {
                debug!("notification sound stream error: {err}");
            }
        })
        .open_sink_or_fallback()
        .map_err(|e| anyhow::anyhow!("open audio stream: {e}"))?;
    sink.log_on_drop(false);
    let player = Player::connect_new(sink.mixer());
    let source = SineWave::new(880.0)
        .take_duration(Duration::from_millis(120))
        .amplify(0.18);
    player.append(source);

    // 120ms 的提示音最多等 600ms。设备异常时采样永远不会被消费，
    // sleep_until_end() 会一直阻塞，所以这里必须带上限。
    let deadline = Instant::now() + Duration::from_millis(600);
    while !player.empty() && !failed.load(Ordering::Relaxed) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    if failed.load(Ordering::Relaxed) {
        anyhow::bail!("audio stream reported an error");
    }
    Ok(())
}

pub fn test_beep() {
    if let Err(err) = play_blocking(true) {
        warn!("test notification sound failed: {err:#}");
    }
}
