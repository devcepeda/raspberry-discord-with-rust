use super::shared::{
    acquire_media_slot, cleanup_path_and_parent, command_failure_details,
    create_persistent_temp_dir, fetch_media_title, run_command_capture, select_final_media_file,
    simplify_yt_dlp_error, voice_join_hint, yt_dlp_available,
};
use once_cell::sync::Lazy;
use serenity::all::{ChannelId, GuildId};
use serenity::async_trait;
use serenity::http::Http;
use serenity::model::channel::Message;
use serenity::prelude::*;
use songbird::events::{
    CoreEvent, Event, EventContext, EventHandler as SongbirdEventHandler, TrackEvent,
};
use songbird::input::{ChildContainer, File, Input};
use songbird::tracks::PlayMode;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{Duration, timeout};

static PLAYBACK_CACHE: Lazy<Mutex<HashMap<String, CachedPlaybackFile>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static GUILD_PLAYBACK_QUEUES: Lazy<Mutex<HashMap<u64, VecDeque<QueuedPlaybackEntry>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static GUILD_VOICE_DEBUG_HOOKS: Lazy<Mutex<HashSet<u64>>> =
    Lazy::new(|| Mutex::new(HashSet::new()));

#[derive(Clone, Debug)]
struct CachedPlaybackFile {
    path: PathBuf,
    refs: usize,
}

#[derive(Clone, Debug)]
struct QueuedPlaybackEntry {
    path: PathBuf,
    title: String,
    url: String,
}

pub(super) async fn queue_status(ctx: &Context, msg: &Message) {
    let Some(guild_id) = msg.guild_id else {
        let _ = msg
            .reply(&ctx.http, "❌ Este comando solo funciona en servidores")
            .await;
        return;
    };

    let snapshot = {
        let queues = GUILD_PLAYBACK_QUEUES.lock().await;
        queues.get(&guild_id.get()).cloned().unwrap_or_default()
    };

    if snapshot.is_empty() {
        let _ = msg.channel_id.say(&ctx.http, "📭 La cola está vacía.").await;
        return;
    }

    let mut lines = Vec::new();
    lines.push(format!("🎶 Cola actual ({} pista(s)):", snapshot.len()));

    for (idx, entry) in snapshot.iter().take(10).enumerate() {
        if idx == 0 {
            lines.push(format!("▶️ {}", entry.title));
        } else {
            lines.push(format!("{}. {}", idx + 1, entry.title));
        }
    }

    if snapshot.len() > 10 {
        lines.push(format!("… y {} más.", snapshot.len() - 10));
    }

    let _ = msg.channel_id.say(&ctx.http, lines.join("\n")).await;
}

pub(super) async fn skip_current(ctx: &Context, msg: &Message) {
    let Some(guild_id) = msg.guild_id else {
        let _ = msg
            .reply(&ctx.http, "❌ Este comando solo funciona en servidores")
            .await;
        return;
    };

    let Some(manager) = songbird::get(ctx).await else {
        let _ = msg
            .channel_id
            .say(&ctx.http, "❌ Songbird no está inicializado")
            .await;
        return;
    };

    let Some(handler_lock) = manager.get(guild_id) else {
        let _ = msg
            .channel_id
            .say(&ctx.http, "No estoy conectado a voz.")
            .await;
        return;
    };

    let queued_before = {
        let queues = GUILD_PLAYBACK_QUEUES.lock().await;
        queues
            .get(&guild_id.get())
            .map(|q| q.len())
            .unwrap_or(0)
    };

    let handler = handler_lock.lock().await;
    let Some(current) = handler.queue().current() else {
        let _ = msg.channel_id.say(&ctx.http, "📭 No hay pistas en cola.").await;
        return;
    };

    let _ = current.stop();
    drop(handler);

    let remaining_hint = queued_before.saturating_sub(1);
    let _ = msg
        .channel_id
        .say(
            &ctx.http,
            format!(
                "⏭️ Saltando pista actual. Quedan {} en cola.",
                remaining_hint
            ),
        )
        .await;
}

pub(super) async fn play_url(ctx: &Context, msg: &Message, url: &str) {
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

    let Some(_permit) = acquire_media_slot(ctx, msg).await else {
        return;
    };

    let media_title = match fetch_media_title(url).await {
        Ok(title) => title,
        Err(details) => {
            let _ = msg
                .channel_id
                .say(
                    &ctx.http,
                    format!("❌ No pude preparar la fuente de audio. {}", details),
                )
                .await;
            return;
        }
    };

    let guild_id = match msg.guild_id {
        Some(g) => g,
        None => {
            let _ = msg
                .reply(&ctx.http, "❌ Este comando solo funciona en servidores")
                .await;
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
            let _ = msg
                .channel_id
                .say(
                    &ctx.http,
                    "⚠️ Debes estar en un canal de voz para reproducir música",
                )
                .await;
            return;
        }
    };

    let manager = match songbird::get(ctx).await {
        Some(m) => m.clone(),
        None => {
            let _ = msg
                .channel_id
                .say(&ctx.http, "❌ Songbird no está inicializado")
                .await;
            return;
        }
    };

    let handler_lock = if let Some(handle) = manager.get(guild_id) {
        eprintln!("[voice] reutilizando handler existente para guild={}.", guild_id.get());
        handle
    } else {
        let join_result = timeout(Duration::from_secs(30), manager.join(guild_id, channel_id)).await;
        match join_result {
            Ok(Ok(h)) => {
                eprintln!(
                    "[voice] join completado guild={} channel={}.",
                    guild_id.get(),
                    channel_id.get()
                );
                h
            }
            Ok(Err(e)) => {
                let raw = format!("{:?}", e);
                eprintln!("Error al unirse al canal: {}", raw);
                let hint = voice_join_hint(&raw);
                let _ = msg
                    .channel_id
                    .say(
                        &ctx.http,
                        format!("❌ No pude unirme al canal de voz. {}", hint),
                    )
                    .await;
                return;
            }
            Err(_) => {
                let _ = msg
                    .channel_id
                    .say(
                        &ctx.http,
                        "❌ Tiempo de espera agotado al conectar a voz. Intenta de nuevo.",
                    )
                    .await;
                return;
            }
        }
    };

    let (playback_audio, cache_hit) = match acquire_cached_playback_audio(url).await {
        Ok(path) => path,
        Err(details) => {
            let _ = msg
                .channel_id
                .say(
                    &ctx.http,
                    format!("❌ No pude preparar el audio para voz. {}", details),
                )
                .await;
            return;
        }
    };

    let playback_input = match build_playback_input(&playback_audio) {
        Ok(input) => input,
        Err(error) => {
            let _ = msg
                .channel_id
                .say(
                    &ctx.http,
                    format!("❌ No pude abrir el audio para reproducirlo. {}", error),
                )
                .await;
            release_cached_playback_audio(&playback_audio).await;
            return;
        }
    };

    let mut handler = handler_lock.lock().await;
    register_voice_debug_hooks_if_needed(guild_id, &mut handler).await;
    let position = handler.queue().len() + 1;
    let track_handle = handler.enqueue_input(playback_input).await;
    // Liberar el lock ANTES de las operaciones async para no bloquear el thread de voz de songbird.
    drop(handler);

    push_guild_playback_entry(
        guild_id,
        QueuedPlaybackEntry {
            path: playback_audio.clone(),
            title: media_title.clone(),
            url: url.to_string(),
        },
    )
    .await;

    eprintln!(
        "[play] guild={} queued position={} cache_hit={} title={:?} path={}",
        guild_id.get(),
        position,
        cache_hit,
        media_title,
        playback_audio.display()
    );

    let _ = track_handle.add_event(
        Event::Track(TrackEvent::Play),
        PlaybackStartNotifier {
            http: ctx.http.clone(),
            channel_id: msg.channel_id,
            guild_id,
            title: media_title.clone(),
        },
    );

    let _ = track_handle.add_event(
        Event::Track(TrackEvent::Error),
        PlaybackCleanupNotifier {
            http: ctx.http.clone(),
            channel_id: msg.channel_id,
            guild_id,
            path: playback_audio.clone(),
            title: media_title.clone(),
            success_text: None,
        },
    );

    let _ = track_handle.add_event(
        Event::Track(TrackEvent::End),
        PlaybackCleanupNotifier {
            http: ctx.http.clone(),
            channel_id: msg.channel_id,
            guild_id,
            path: playback_audio.clone(),
            title: media_title.clone(),
            success_text: Some("ℹ️ La reproducción terminó."),
        },
    );

    let status_text = if position == 1 {
        format!("🎵 Reproduciendo: {}", media_title)
    } else {
        format!("📝 Añadido a la cola (#{position}): {}", media_title)
    };

    let _ = msg.channel_id.say(&ctx.http, status_text).await;
}

async fn acquire_cached_playback_audio(url: &str) -> Result<(PathBuf, bool), String> {
    let mut cache = PLAYBACK_CACHE.lock().await;
    if let Some(entry) = cache.get_mut(url) {
        if entry.path.exists() {
            entry.refs += 1;
            eprintln!(
                "[play-cache] hit url={} refs={} path={}",
                url,
                entry.refs,
                entry.path.display()
            );
            return Ok((entry.path.clone(), true));
        }

        eprintln!(
            "[play-cache] stale entry removed url={} path={}",
            url,
            entry.path.display()
        );
    }
    cache.remove(url);
    drop(cache);

    let path = download_playback_audio(url).await?;

    let mut cache = PLAYBACK_CACHE.lock().await;
    cache.insert(
        url.to_string(),
        CachedPlaybackFile {
            path: path.clone(),
            refs: 1,
        },
    );
    eprintln!("[play-cache] miss url={} path={}", url, path.display());

    Ok((path, false))
}

async fn release_cached_playback_audio(path: &std::path::Path) {
    let mut cache = PLAYBACK_CACHE.lock().await;
    let cache_key = cache
        .iter()
        .find_map(|(url, entry)| (entry.path == path).then(|| url.clone()));

    let mut cleanup_path = None;
    if let Some(cache_key) = cache_key {
        if let Some(entry) = cache.get_mut(&cache_key) {
            if entry.refs > 1 {
                entry.refs -= 1;
                eprintln!(
                    "[play-cache] release url={} refs={} path={}",
                    cache_key,
                    entry.refs,
                    entry.path.display()
                );
            } else {
                cleanup_path = Some(entry.path.clone());
            }
        }

        if cleanup_path.is_some() {
            cache.remove(&cache_key);
        }
    }
    drop(cache);

    if let Some(cleanup_path) = cleanup_path {
        eprintln!("[play-cache] evict path={}", cleanup_path.display());
        let _ = cleanup_path_and_parent(&cleanup_path);
    }
}

async fn push_guild_playback_entry(guild_id: GuildId, entry: QueuedPlaybackEntry) {
    let mut queues = GUILD_PLAYBACK_QUEUES.lock().await;
    queues.entry(guild_id.get()).or_default().push_back(entry);
}

async fn finish_guild_playback_path(guild_id: GuildId, path: &std::path::Path) {
    let mut queues = GUILD_PLAYBACK_QUEUES.lock().await;
    let removed = if let Some(queue) = queues.get_mut(&guild_id.get()) {
        if let Some(position) = queue.iter().position(|entry| entry.path == path) {
            let removed = queue.remove(position);
            if queue.is_empty() {
                queues.remove(&guild_id.get());
            }
            removed
        } else {
            None
        }
    } else {
        None
    };
    drop(queues);

    if let Some(removed) = removed {
        eprintln!(
            "[play-queue] removing title={:?} url={} path={}",
            removed.title,
            removed.url,
            removed.path.display()
        );
        release_cached_playback_audio(&removed.path).await;
    }
}

pub(super) async fn clear_guild_playback_queue(guild_id: GuildId) {
    let entries = {
        let mut queues = GUILD_PLAYBACK_QUEUES.lock().await;
        queues.remove(&guild_id.get()).unwrap_or_default()
    };

    for entry in entries {
        release_cached_playback_audio(&entry.path).await;
    }
}

pub(super) async fn clear_guild_voice_debug_hooks(guild_id: GuildId) {
    let mut hooks = GUILD_VOICE_DEBUG_HOOKS.lock().await;
    hooks.remove(&guild_id.get());
}

async fn register_voice_debug_hooks_if_needed(guild_id: GuildId, handler: &mut songbird::Call) {
    let should_register = {
        let mut hooks = GUILD_VOICE_DEBUG_HOOKS.lock().await;
        hooks.insert(guild_id.get())
    };

    if !should_register {
        return;
    }

    handler.add_global_event(
        Event::Core(CoreEvent::DriverConnect),
        VoiceDebugNotifier {
            guild_id,
            label: "connect",
        },
    );
    handler.add_global_event(
        Event::Core(CoreEvent::DriverReconnect),
        VoiceDebugNotifier {
            guild_id,
            label: "reconnect",
        },
    );
    handler.add_global_event(
        Event::Core(CoreEvent::DriverDisconnect),
        VoiceDebugNotifier {
            guild_id,
            label: "disconnect",
        },
    );
}

async fn download_playback_audio(url: &str) -> Result<PathBuf, String> {
    let temp_dir = create_persistent_temp_dir("play-")?;
    let output_template = temp_dir.join("%(title)s.%(ext)s");

    eprintln!("[play-download] starting url={} temp_dir={}", url, temp_dir.display());

    let primary = run_playback_download(url, &output_template, false).await;
    let output = match primary {
        Ok(out) if out.status.success() => out,
        Ok(out) => {
            let details = command_failure_details(&out);
            eprintln!(
                "[play-download] primary yt-dlp failed url={} details={}",
                url, details
            );

            let fallback = run_playback_download(url, &output_template, true).await;
            match fallback {
                Ok(fallback_out) if fallback_out.status.success() => fallback_out,
                Ok(fallback_out) => {
                    let fallback_details = command_failure_details(&fallback_out);
                    eprintln!(
                        "[play-download] fallback yt-dlp failed url={} details={}",
                        url, fallback_details
                    );
                    let _ = std::fs::remove_dir_all(&temp_dir);
                    return Err(simplify_yt_dlp_error(&fallback_details));
                }
                Err(error) => {
                    eprintln!(
                        "[play-download] fallback execution failed url={} error={}",
                        url, error
                    );
                    let _ = std::fs::remove_dir_all(&temp_dir);
                    return Err(error);
                }
            }
        }
        Err(error) => {
            eprintln!("[play-download] execution failed url={} error={}", url, error);
            let fallback = run_playback_download(url, &output_template, true).await;
            match fallback {
                Ok(fallback_out) if fallback_out.status.success() => fallback_out,
                Ok(fallback_out) => {
                    let fallback_details = command_failure_details(&fallback_out);
                    eprintln!(
                        "[play-download] fallback yt-dlp failed url={} details={}",
                        url, fallback_details
                    );
                    let _ = std::fs::remove_dir_all(&temp_dir);
                    return Err(simplify_yt_dlp_error(&fallback_details));
                }
                Err(fallback_error) => {
                    eprintln!(
                        "[play-download] fallback execution failed url={} error={}",
                        url, fallback_error
                    );
                    let _ = std::fs::remove_dir_all(&temp_dir);
                    return Err(fallback_error);
                }
            }
        }
    };

    let Some(audio_file) = select_playback_audio_file(&temp_dir) else {
        eprintln!(
            "[play-download] no final audio found url={} output={}",
            url,
            command_failure_details(&output)
        );
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err("La descarga terminó incompleta: no encontré el audio final.".to_string());
    };

    eprintln!(
        "[play-download] ready url={} path={} size_bytes={}",
        url,
        audio_file.display(),
        std::fs::metadata(&audio_file).map(|m| m.len()).unwrap_or(0)
    );

    Ok(audio_file)
}

async fn run_playback_download(
    url: &str,
    output_template: &std::path::Path,
    extract_audio: bool,
) -> Result<std::process::Output, String> {
    let mut args = vec![
        "--no-playlist".to_string(),
        "--no-warnings".to_string(),
        "--restrict-filenames".to_string(),
        "--concurrent-fragments".to_string(),
        "1".to_string(),
        "-f".to_string(),
        "ba[ext=m4a]/ba[ext=webm]/ba[ext=opus]/ba/b".to_string(),
    ];

    if extract_audio {
        args.push("-x".to_string());
        args.push("--audio-format".to_string());
        args.push("best".to_string());
    }

    args.push("-o".to_string());
    args.push(output_template.to_string_lossy().to_string());
    args.push(url.to_string());

    run_command_capture("yt-dlp", args).await
}

fn build_playback_input(path: &Path) -> Result<Input, String> {
    if prefer_direct_playback(path) {
        return Ok(File::new(path.to_path_buf()).into());
    }

    let child = Command::new("ffmpeg")
        .arg("-nostdin")
        .arg("-v")
        .arg("error")
        .arg("-i")
        .arg(path)
        .arg("-vn")
        .arg("-sn")
        .arg("-dn")
        .arg("-map_metadata")
        .arg("-1")
        .arg("-ac")
        .arg("2")
        .arg("-ar")
        .arg("48000")
        .arg("-f")
        .arg("wav")
        .arg("-")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("ffmpeg no pudo preparar el audio: {}", e))?;

    Ok(ChildContainer::from(child).into())
}

fn prefer_direct_playback(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some(ext) if ext.eq_ignore_ascii_case("opus")
            || ext.eq_ignore_ascii_case("ogg")
            || ext.eq_ignore_ascii_case("webm")
    )
}

fn select_playback_audio_file(dir: &std::path::Path) -> Option<PathBuf> {
    if let Some(path) = select_final_media_file(dir, &["m4a", "webm", "opus", "ogg", "mp3", "aac", "mp4"]) {
        return Some(path);
    }

    let mut candidates: Vec<(PathBuf, std::time::SystemTime)> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_file() {
                return None;
            }

            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            if name.ends_with(".part") || name.ends_with(".tmp") || name.ends_with(".ytdl") {
                return None;
            }

            let modified = std::fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

            Some((path, modified))
        })
        .collect();

    candidates.sort_by(|a, b| b.1.cmp(&a.1));
    candidates.into_iter().next().map(|(path, _)| path)
}

struct PlaybackStartNotifier {
    http: Arc<Http>,
    channel_id: ChannelId,
    guild_id: GuildId,
    title: String,
}

#[async_trait]
impl SongbirdEventHandler for PlaybackStartNotifier {
    async fn act(&self, _ctx: &EventContext<'_>) -> Option<Event> {
        eprintln!(
            "[play] track started guild={} title={:?}",
            self.guild_id.get(),
            self.title
        );
        let _ = self
            .channel_id
            .say(&self.http, format!("✅ Audio iniciado: {}", self.title))
            .await;
        None
    }
}

struct PlaybackCleanupNotifier {
    http: Arc<Http>,
    channel_id: ChannelId,
    guild_id: GuildId,
    path: PathBuf,
    title: String,
    success_text: Option<&'static str>,
}

#[async_trait]
impl SongbirdEventHandler for PlaybackCleanupNotifier {
    async fn act(&self, ctx: &EventContext<'_>) -> Option<Event> {
        let message = match ctx {
            EventContext::Track([(state, _)]) => match &state.playing {
                PlayMode::Errored(err) => {
                    eprintln!(
                        "[play] track error guild={} title={:?} path={} error={}",
                        self.guild_id.get(),
                        self.title,
                        self.path.display(),
                        err
                    );
                    Some(format!("❌ La pista falló al reproducirse. {}", err))
                }
                _ => self.success_text.map(|text| text.to_string()),
            },
            _ => self.success_text.map(|text| text.to_string()),
        };

        if self.success_text.is_some() {
            eprintln!(
                "[play] track ended guild={} title={:?} path={}",
                self.guild_id.get(),
                self.title,
                self.path.display()
            );
        }

        finish_guild_playback_path(self.guild_id, &self.path).await;

        if let Some(message) = message {
            let _ = self.channel_id.say(&self.http, message).await;
        }

        None
    }
}

struct VoiceDebugNotifier {
    guild_id: GuildId,
    label: &'static str,
}

#[async_trait]
impl SongbirdEventHandler for VoiceDebugNotifier {
    async fn act(&self, ctx: &EventContext<'_>) -> Option<Event> {
        match ctx {
            EventContext::DriverConnect(data) => {
                eprintln!(
                    "[voice] {} guild={} channel={} server={} ssrc={} session={}",
                    self.label,
                    data.guild_id.0.get(),
                    data.channel_id.0.get(),
                    data.server,
                    data.ssrc,
                    data.session_id
                );
            }
            EventContext::DriverReconnect(data) => {
                eprintln!(
                    "[voice] {} guild={} channel={} server={} ssrc={} session={}",
                    self.label,
                    data.guild_id.0.get(),
                    data.channel_id.0.get(),
                    data.server,
                    data.ssrc,
                    data.session_id
                );
            }
            EventContext::DriverDisconnect(data) => {
                eprintln!(
                    "[voice] {} guild={} channel={} kind={:?} reason={:?} session={}",
                    self.label,
                    data.guild_id.0.get(),
                    data.channel_id.0.get(),
                    data.kind,
                    data.reason,
                    data.session_id
                );
            }
            _ => {
                eprintln!("[voice] {} guild={} unknown context", self.label, self.guild_id.get());
            }
        }

        None
    }
}
