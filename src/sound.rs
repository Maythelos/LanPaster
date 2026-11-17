use rodio::{source::SineWave, DeviceSinkBuilder, Player, Source};
use std::time::Duration;
use tracing::{debug, warn};

/// Play a short notification beep on a background thread.
pub fn play() {
    let _ = std::thread::Builder::new()
        .name("lanpaster-sound".to_string())
        .spawn(|| {
            if let Err(err) = play_blocking() {
                debug!("notification sound failed: {err:#}");
            }
        });
}

fn play_blocking() -> anyhow::Result<()> {
    let mut sink = DeviceSinkBuilder::open_default_sink()
        .map_err(|e| anyhow::anyhow!("open audio device: {e}"))?;
    sink.log_on_drop(false);
    let player = Player::connect_new(&sink.mixer());
    let source = SineWave::new(880.0)
        .take_duration(Duration::from_millis(120))
        .amplify(0.18);
    player.append(source);
    player.sleep_until_end();
    Ok(())
}

pub fn test_beep() {
    if let Err(err) = play_blocking() {
        warn!("test notification sound failed: {err:#}");
    }
}
