use once_cell::sync::Lazy;
use serenity::all::{CreateAttachment, CreateMessage};
use serenity::model::channel::Message;
use serenity::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::TempDir;
use tokio::process::Command;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::{Duration, timeout};

static MEDIA_JOBS: Lazy<Arc<Semaphore>> = Lazy::new(|| Arc::new(Semaphore::new(1)));
static PENDING_MEDIA_JOBS: AtomicUsize = AtomicUsize::new(0);

pub(super) const DISCORD_UPLOAD_LIMIT_BYTES: u64 = 20 * 1024 * 1024;
const COMMAND_TIMEOUT_SECS: u64 = 600;
/// Timeout para operaciones de encode/transcode en la Pi (puede tardar mucho con libx264).
const FFMPEG_ENCODE_TIMEOUT_SECS: u64 = 1800;
const MEDIA_QUEUE_WAIT_SECS: u64 = 1800;
/// 2 hilos para encode real; usa la mitad de los 4 cores del Pi sin saturarlo.
const FFMPEG_ENCODE_THREADS: &str = "2";
const TEMP_DIR_PREFIX: &str = "discord-bot-media-";

pub(super) async fn send_file(
    ctx: &Context,
    msg: &Message,
    path: &Path,
    caption: &str,
) -> serenity::Result<Message> {
    let attachment = CreateAttachment::path(path).await?;
    msg.channel_id
        .send_files(
            &ctx.http,
            vec![attachment],
            CreateMessage::new().content(caption),
        )
        .await
}

pub(super) async fn ensure_mp3_under_limit(
    input: &Path,
    temp_root: &Path,
) -> Result<PathBuf, String> {
    let input_size = file_size(input)?;
    if input_size <= DISCORD_UPLOAD_LIMIT_BYTES {
        return Ok(input.to_path_buf());
    }

    let duration = media_duration_seconds(input).await?;
    let base = target_bitrate_kbps(duration, 32)?;
    let attempts = bitrate_attempts(base, 96, 320);
    let title_stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("audio");

    for (idx, bitrate) in attempts.iter().enumerate() {
        let output_path = temp_root.join(format!("{}-compressed-{}.mp3", title_stem, idx + 1));
        let output = run_command_capture_with_timeout(
            "ffmpeg",
            vec![
                "-y".to_string(),
                "-i".to_string(),
                input.to_string_lossy().to_string(),
                "-vn".to_string(),
                "-c:a".to_string(),
                "libmp3lame".to_string(),
                "-b:a".to_string(),
                format!("{}k", bitrate),
                "-threads".to_string(),
                FFMPEG_ENCODE_THREADS.to_string(),
                output_path.to_string_lossy().to_string(),
            ],
            FFMPEG_ENCODE_TIMEOUT_SECS,
        )
        .await?;

        if output.status.success()
            && file_size(&output_path).unwrap_or(u64::MAX) <= DISCORD_UPLOAD_LIMIT_BYTES
        {
            return Ok(output_path);
        }
    }

    Err("El MP3 sigue superando 20MB incluso tras compresión.".to_string())
}

pub(super) async fn ensure_video_under_limit(
    input: &Path,
    temp_root: &Path,
) -> Result<PathBuf, String> {
    let input_size = file_size(input)?;
    if input_size <= DISCORD_UPLOAD_LIMIT_BYTES {
        return Ok(input.to_path_buf());
    }

    let duration = media_duration_seconds(input).await?;
    let total_kbps = target_bitrate_kbps(duration, 96)?;
    let audio_kbps = if total_kbps > 192 { 160 } else { 96 };
    let video_base = total_kbps.saturating_sub(audio_kbps).max(250);
    let attempts = bitrate_attempts(video_base, 220, video_base);
    let title_stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("video");

    for (idx, bitrate) in attempts.iter().enumerate() {
        let output_path = temp_root.join(format!("{}-compressed-{}.mp4", title_stem, idx + 1));
        let output = run_command_capture_with_timeout(
            "ffmpeg",
            vec![
                "-y".to_string(),
                "-i".to_string(),
                input.to_string_lossy().to_string(),
                "-c:v".to_string(),
                "libx264".to_string(),
                // ultrafast es ~3x más rápido que veryfast en Pi; el archivo queda algo más
                // grande pero ya lo comprimimos por bitrate de todas formas.
                "-preset".to_string(),
                "ultrafast".to_string(),
                "-tune".to_string(),
                "fastdecode".to_string(),
                "-pix_fmt".to_string(),
                "yuv420p".to_string(),
                "-b:v".to_string(),
                format!("{}k", bitrate),
                "-maxrate".to_string(),
                format!("{}k", bitrate),
                "-bufsize".to_string(),
                format!("{}k", bitrate.saturating_mul(2)),
                "-c:a".to_string(),
                "aac".to_string(),
                "-b:a".to_string(),
                format!("{}k", audio_kbps),
                "-movflags".to_string(),
                "+faststart".to_string(),
                "-max_muxing_queue_size".to_string(),
                "2048".to_string(),
                "-threads".to_string(),
                FFMPEG_ENCODE_THREADS.to_string(),
                output_path.to_string_lossy().to_string(),
            ],
            FFMPEG_ENCODE_TIMEOUT_SECS,
        )
        .await?;

        if output.status.success()
            && file_size(&output_path).unwrap_or(u64::MAX) <= DISCORD_UPLOAD_LIMIT_BYTES
        {
            return Ok(output_path);
        }
    }

    Err("El video sigue superando 20MB incluso tras compresión.".to_string())
}

pub(super) fn create_temp_dir() -> Result<TempDir, String> {
    // Use /var/tmp (disk-backed, ~89 GB free) instead of /tmp (923 MB RAM-backed tmpfs).
    let base = std::path::Path::new("/var/tmp");
    tempfile::Builder::new()
        .prefix(TEMP_DIR_PREFIX)
        .tempdir_in(base)
        .map_err(|e| format!("no pude crear carpeta temporal: {}", e))
}

pub(super) fn create_persistent_temp_dir(tag: &str) -> Result<PathBuf, String> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("no pude calcular tiempo actual: {}", e))?
        .as_millis();
    let dir = Path::new("/var/tmp").join(format!("{}{}-{}", TEMP_DIR_PREFIX, tag, millis));
    fs::create_dir_all(&dir).map_err(|e| format!("no pude crear carpeta temporal: {}", e))?;
    Ok(dir)
}

pub(super) fn select_final_media_file(dir: &Path, preferred_exts: &[&str]) -> Option<PathBuf> {
    let mut candidates: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();

    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if name.ends_with(".part") || name.ends_with(".tmp") || name.ends_with(".ytdl") {
            continue;
        }

        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default();
        if !preferred_exts
            .iter()
            .any(|candidate| ext.eq_ignore_ascii_case(candidate))
        {
            continue;
        }

        let modified = fs::metadata(&path)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

        candidates.push((path, modified));
    }

    candidates.sort_by(|a, b| b.1.cmp(&a.1));
    candidates.into_iter().next().map(|(path, _)| path)
}

pub(super) fn cleanup_temp_dir_contents(dir: &Path) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("no pude listar temporales: {}", e))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            let _ = fs::remove_file(path);
        } else if path.is_dir() {
            let _ = fs::remove_dir_all(path);
        }
    }
    Ok(())
}

pub(super) fn cleanup_path_and_parent(path: &Path) -> Result<(), String> {
    if path.is_file() {
        let _ = fs::remove_file(path);
    }

    if let Some(parent) = path.parent() {
        let _ = cleanup_temp_dir_contents(parent);
        let _ = fs::remove_dir_all(parent);
    }

    Ok(())
}

/// Removes any leftover discord-bot temp dirs from both /var/tmp and /tmp.
pub(super) fn purge_previous_temp_dirs() -> Result<(), String> {
    for temp_root in &[
        std::path::PathBuf::from("/var/tmp"),
        std::env::temp_dir(),
    ] {
        let Ok(entries) = fs::read_dir(temp_root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.starts_with(TEMP_DIR_PREFIX) {
                let _ = fs::remove_dir_all(path);
            }
        }
    }
    Ok(())
}

pub(super) async fn acquire_media_slot(
    ctx: &Context,
    msg: &Message,
) -> Option<OwnedSemaphorePermit> {
    let in_progress = if MEDIA_JOBS.available_permits() == 0 {
        1
    } else {
        0
    };
    let waiting_before = PENDING_MEDIA_JOBS.fetch_add(1, Ordering::SeqCst);
    let ahead = waiting_before + in_progress;

    if ahead > 0 {
        let _ = msg
            .channel_id
            .say(
                &ctx.http,
                format!("⏳ Solicitud en cola. Hay {} trabajo(s) antes del tuyo.", ahead),
            )
            .await;
    }

    let acquire_result = timeout(
        Duration::from_secs(MEDIA_QUEUE_WAIT_SECS),
        MEDIA_JOBS.clone().acquire_owned(),
    )
    .await;

    PENDING_MEDIA_JOBS.fetch_sub(1, Ordering::SeqCst);

    match acquire_result {
        Ok(Ok(permit)) => Some(permit),
        Ok(Err(_)) => {
            let _ = msg
                .channel_id
                .say(
                    &ctx.http,
                    "❌ No pude reservar el procesador de medios. Intenta nuevamente.",
                )
                .await;
            None
        }
        Err(_) => {
            let _ = msg
                .channel_id
                .say(
                    &ctx.http,
                    "⌛ La solicitud esperó demasiado en cola. Vuelve a intentarlo.",
                )
                .await;
            None
        }
    }
}


pub(super) fn file_size(path: &Path) -> Result<u64, String> {
    fs::metadata(path)
        .map(|m| m.len())
        .map_err(|e| format!("no pude leer tamaño del archivo: {}", e))
}

fn bitrate_attempts(base: u64, min: u64, max: u64) -> Vec<u64> {
    let mut values = Vec::new();
    let factors = [1.0_f64, 0.85_f64, 0.70_f64];
    for factor in factors {
        let value = ((base as f64) * factor).round() as u64;
        let clamped = value.clamp(min, max);
        if !values.contains(&clamped) {
            values.push(clamped);
        }
    }
    values
}

fn target_bitrate_kbps(duration_secs: f64, safety_kbps: u64) -> Result<u64, String> {
    if !duration_secs.is_finite() || duration_secs <= 0.0 {
        return Err("No pude calcular la duración del archivo para comprimirlo.".to_string());
    }

    let bits_total = (DISCORD_UPLOAD_LIMIT_BYTES as f64) * 8.0;
    let kbps = (bits_total / duration_secs / 1000.0).floor() as i64 - (safety_kbps as i64);
    if kbps <= 64 {
        return Err("El contenido es demasiado largo para mantener calidad dentro de 20MB.".to_string());
    }

    Ok(kbps as u64)
}

pub(super) async fn media_duration_seconds(path: &Path) -> Result<f64, String> {
    let output = run_command_capture(
        "ffprobe",
        vec![
            "-v".to_string(),
            "error".to_string(),
            "-show_entries".to_string(),
            "format=duration".to_string(),
            "-of".to_string(),
            "default=nokey=1:noprint_wrappers=1".to_string(),
            path.to_string_lossy().to_string(),
        ],
    )
    .await?;

    if !output.status.success() {
        return Err("ffprobe no pudo leer la duración del archivo.".to_string());
    }

    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    raw.parse::<f64>()
        .map_err(|_| "ffprobe devolvió una duración inválida.".to_string())
}

pub(super) async fn run_command_capture(program: &str, args: Vec<String>) -> Result<Output, String> {
    run_command_capture_with_timeout(program, args, COMMAND_TIMEOUT_SECS).await
}

pub(super) async fn run_command_capture_with_timeout(
    program: &str,
    args: Vec<String>,
    timeout_secs: u64,
) -> Result<Output, String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = timeout(Duration::from_secs(timeout_secs), command.output())
        .await
        .map_err(|_| format!("{} tardó demasiado en finalizar (límite: {}s)", program, timeout_secs))?
        .map_err(|e| format!("no pude ejecutar {}: {}", program, e))?;

    Ok(output)
}

pub(super) fn output_has_postprocessing_failure(output: &Output) -> bool {
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    )
    .to_ascii_lowercase();
    combined.contains("conversion failed") || combined.contains("postprocessing:")
}

pub(super) fn command_failure_details(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    stderr
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .or_else(|| stdout.lines().rev().find(|line| !line.trim().is_empty()))
        .unwrap_or("comando terminó con error sin detalles")
        .to_string()
}

pub(super) fn voice_join_hint(details: &str) -> &'static str {
    if details.contains("4017") || details.contains("DAVE") || details.contains("E2EE") {
        "Discord exige el protocolo de voz más reciente; reinicia el bot con la versión actual y verifica permisos Connect/Speak."
    } else {
        "Verifica permisos Connect/Speak y que el canal no esté lleno o restringido."
    }
}

pub(super) async fn fetch_media_title(url: &str) -> Result<String, String> {
    let output = run_command_capture(
        "yt-dlp",
        vec![
            "--no-playlist".to_string(),
            "--skip-download".to_string(),
            "--print".to_string(),
            "title".to_string(),
            url.to_string(),
        ],
    )
    .await?;

    if !output.status.success() {
        return Err(simplify_yt_dlp_error(&command_failure_details(&output)));
    }

    let title = String::from_utf8_lossy(&output.stdout)
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_string())
        .unwrap_or_else(|| "Audio de YouTube".to_string());

    Ok(title)
}

pub(super) async fn yt_dlp_available() -> bool {
    command_available("yt-dlp").await
}

pub(super) async fn ffmpeg_available() -> bool {
    command_available("ffmpeg").await
}

pub(super) async fn ffprobe_available() -> bool {
    command_available("ffprobe").await
}

async fn command_available(program: &str) -> bool {
    let version_flag = if program == "yt-dlp" {
        "--version"
    } else {
        "-version"
    };

    let mut command = Command::new(program);
    command
        .arg(version_flag)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    match timeout(Duration::from_secs(5), command.status()).await {
        Ok(Ok(status)) => status.success(),
        _ => false,
    }
}

pub(super) fn simplify_yt_dlp_error(details: &str) -> String {
    let lower = details.to_ascii_lowercase();

    if lower.contains("no space left on device") || lower.contains("errno 28") {
        "Sin espacio en disco para descargar el archivo. Intenta de nuevo en un momento.".to_string()
    } else if lower.contains("requested format is not available") {
        "El formato solicitado no está disponible para ese contenido o región.".to_string()
    } else if lower.contains("video unavailable") {
        "El video no está disponible para tu región, cuenta o política del proveedor.".to_string()
    } else if lower.contains("file is larger than max-filesize")
        || lower.contains("exceeds max_filesize")
    {
        "El contenido excede 20MB incluso tras los filtros iniciales.".to_string()
    } else if lower.contains("sign in to confirm") || lower.contains("login required") {
        "YouTube pidió autenticación para ese contenido.".to_string()
    } else if lower.contains("conversion failed") {
        "Falló la conversión del formato. Reintentando con modo compatible.".to_string()
    } else {
        format!("Detalle: {}", details)
    }
}
