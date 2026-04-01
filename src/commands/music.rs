use once_cell::sync::Lazy;
use serenity::all::{CreateAttachment, CreateMessage};
use serenity::model::channel::Message;
use serenity::prelude::*;
use songbird::input::YoutubeDl;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::process::Command;

static HTTP_CLIENT: Lazy<reqwest::Client> = Lazy::new(reqwest::Client::new);
const DISCORD_UPLOAD_LIMIT_BYTES: u64 = 8 * 1024 * 1024;

pub async fn run(ctx: &Context, msg: &Message) {
    if !msg.content.starts_with('!') {
        return;
    }

    if msg.content == "!stop" {
        stop(ctx, msg).await;
        return;
    }

    if msg.content == "!leave" {
        leave(ctx, msg).await;
        return;
    }

    if let Some(url) = parse_arg(&msg.content, "!ytdownload") {
        download_and_send(ctx, msg, url).await;
        return;
    }

    if let Some(url) = parse_arg(&msg.content, "!play") {
        play_url(ctx, msg, url).await;
        return;
    }

    if let Some(url) = parse_arg(&msg.content, "!yt") {
        play_url(ctx, msg, url).await;
    }
}

fn parse_arg<'a>(content: &'a str, command: &str) -> Option<&'a str> {
    let trimmed = content.trim();
    let remainder = trimmed.strip_prefix(command)?;
    if !remainder.is_empty() && !remainder.starts_with(char::is_whitespace) {
        return None;
    }

    let rest = remainder.trim();
    if rest.is_empty() {
        return None;
    }

    Some(rest)
}

async fn play_url(ctx: &Context, msg: &Message, url: &str) {
    println!("Comando play detectado: {}", msg.content);

    if !yt_dlp_available().await {
        let _ = msg
            .channel_id
            .say(
                &ctx.http,
                "❌ Falta yt-dlp en el servidor. Instala yt-dlp para usar !play/!yt.",
            )
            .await;
        return;
    }

    let guild_id = match msg.guild_id {
        Some(g) => g,
        None => {
            let _ = msg.reply(&ctx.http, "❌ Este comando solo funciona en servidores").await;
            return;
        }
    };

    let channel_id_opt = {
        if let Some(guild_cache) = ctx.cache.guild(guild_id) {
            guild_cache
                .voice_states
                .get(&msg.author.id)
                .and_then(|vs| vs.channel_id)
        } else {
            None
        }
    };

    let channel_id = match channel_id_opt {
        Some(cid) => cid,
        None => {
            let _ = msg.channel_id
                .say(&ctx.http, "⚠️ Debes estar en un canal de voz para reproducir música")
                .await;
            return;
        }
    };

    let manager = match songbird::get(ctx).await {
        Some(m) => m.clone(),
        None => {
            let _ = msg.channel_id.say(&ctx.http, "❌ Songbird no está inicializado").await;
            return;
        }
    };

    let handler_lock = match manager.get(guild_id) {
        Some(handle) => handle,
        None => match manager.join(guild_id, channel_id).await {
            Ok(h) => h,
            Err(e) => {
                eprintln!("Error al unirse al canal: {:?}", e);
                let _ = msg.channel_id.say(&ctx.http, "❌ No pude unirme al canal de voz").await;
                return;
            }
        },
    };

    let mut handler = handler_lock.lock().await;

    let source = YoutubeDl::new(HTTP_CLIENT.clone(), url.to_string());
    let _track_handle = handler.play_input(source.into());

    let _ = msg
        .channel_id
        .say(&ctx.http, format!("🎵 Reproduciendo: {}", url))
        .await;
}

async fn stop(ctx: &Context, msg: &Message) {
    let Some(guild_id) = msg.guild_id else {
        return;
    };

    let Some(manager) = songbird::get(ctx).await else {
        return;
    };

    if let Some(handler_lock) = manager.get(guild_id) {
        let mut handler = handler_lock.lock().await;
        handler.stop();
        let _ = msg.channel_id.say(&ctx.http, "⏹️ Reproducción detenida.").await;
    } else {
        let _ = msg.channel_id.say(&ctx.http, "No estoy conectado a voz.").await;
    }
}

async fn leave(ctx: &Context, msg: &Message) {
    let Some(guild_id) = msg.guild_id else {
        return;
    };

    let Some(manager) = songbird::get(ctx).await else {
        return;
    };

    if manager.get(guild_id).is_some() {
        let _ = manager.remove(guild_id).await;
        let _ = msg.channel_id.say(&ctx.http, "👋 Salí del canal de voz.").await;
    } else {
        let _ = msg.channel_id.say(&ctx.http, "No estoy conectado a voz.").await;
    }
}

async fn download_and_send(ctx: &Context, msg: &Message, url: &str) {
    if !yt_dlp_available().await {
        let _ = msg
            .channel_id
            .say(&ctx.http, "❌ Falta yt-dlp en el servidor. Instala yt-dlp para usar descargas.")
            .await;
        return;
    }

    let _ = msg
        .channel_id
        .say(&ctx.http, "⬇️ Descargando video de YouTube...")
        .await;

    let temp_dir = create_temp_dir();
    if let Err(e) = fs::create_dir_all(&temp_dir) {
        let _ = msg
            .channel_id
            .say(&ctx.http, format!("❌ No pude crear carpeta temporal: {}", e))
            .await;
        return;
    }

    let output_template = temp_dir.join("video.%(ext)s");
    let max_size_arg = format!("{}", DISCORD_UPLOAD_LIMIT_BYTES);

    let output = Command::new("yt-dlp")
        .arg("--no-playlist")
        .arg("--no-warnings")
        .arg("--restrict-filenames")
        .arg("--merge-output-format")
        .arg("mp4")
        .arg("--max-filesize")
        .arg(max_size_arg)
        .arg("-f")
        .arg("bv*[ext=mp4]+ba[ext=m4a]/b[ext=mp4]/b")
        .arg("-o")
        .arg(output_template.to_string_lossy().to_string())
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;

    let Ok(output) = output else {
        let _ = msg
            .channel_id
            .say(&ctx.http, "❌ Falló la ejecución de yt-dlp.")
            .await;
        let _ = fs::remove_dir_all(&temp_dir);
        return;
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let details = stderr
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .or_else(|| stdout.lines().rev().find(|line| !line.trim().is_empty()))
            .unwrap_or("yt-dlp terminó con error sin detalles");

        eprintln!("yt-dlp falló para {}: {}", url, details);

        let _ = msg
            .channel_id
            .say(
                &ctx.http,
                format!(
                    "❌ No pude descargar el video. {}",
                    simplify_yt_dlp_error(details)
                ),
            )
            .await;
        let _ = fs::remove_dir_all(&temp_dir);
        return;
    }

    let Some(video_path) = first_file_in_dir(&temp_dir) else {
        let _ = msg
            .channel_id
            .say(&ctx.http, "❌ Descarga incompleta: no encontré el archivo final.")
            .await;
        let _ = fs::remove_dir_all(&temp_dir);
        return;
    };

    match fs::metadata(&video_path) {
        Ok(meta) if meta.len() > DISCORD_UPLOAD_LIMIT_BYTES => {
            let _ = msg
                .channel_id
                .say(
                    &ctx.http,
                    "❌ El archivo final supera el límite de subida de Discord (8MB).",
                )
                .await;
            let _ = fs::remove_dir_all(&temp_dir);
            return;
        }
        Ok(_) => {}
        Err(_) => {
            let _ = msg
                .channel_id
                .say(&ctx.http, "❌ No pude leer el archivo descargado.")
                .await;
            let _ = fs::remove_dir_all(&temp_dir);
            return;
        }
    }

    let attachment = match CreateAttachment::path(&video_path).await {
        Ok(file) => file,
        Err(_) => {
            let _ = msg
                .channel_id
                .say(&ctx.http, "❌ No pude preparar el video para enviarlo.")
                .await;
            let _ = fs::remove_dir_all(&temp_dir);
            return;
        }
    };

    let sent = msg
        .channel_id
        .send_files(
            &ctx.http,
            vec![attachment],
            CreateMessage::new().content("📦 Video solicitado"),
        )
        .await;

    if sent.is_err() {
        let _ = msg
            .channel_id
            .say(
                &ctx.http,
                "❌ No pude enviar el video. Verifica permisos de adjuntos en este canal.",
            )
            .await;
    }

    let _ = fs::remove_dir_all(&temp_dir);
}

fn create_temp_dir() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    std::env::temp_dir().join(format!("discord-bot-yt-{}", nonce))
}

fn first_file_in_dir(dir: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

async fn yt_dlp_available() -> bool {
    let status = Command::new("yt-dlp")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;

    matches!(status, Ok(s) if s.success())
}

fn simplify_yt_dlp_error(details: &str) -> String {
    let lower = details.to_ascii_lowercase();

    if lower.contains("requested format is not available") {
        "El formato solicitado no está disponible para ese video.".to_string()
    } else if lower.contains("file is larger than max-filesize") || lower.contains("exceeds max_filesize") {
        "El archivo excede el límite de subida de Discord (8MB).".to_string()
    } else if lower.contains("sign in to confirm") || lower.contains("login required") {
        "YouTube pidió autenticación para ese contenido.".to_string()
    } else {
        format!("Detalle: {}", details)
    }
}