use super::shared::{HTTP_CLIENT, fetch_media_title, voice_join_hint, yt_dlp_available};
use serenity::all::ChannelId;
use serenity::async_trait;
use serenity::http::Http;
use serenity::model::channel::Message;
use serenity::prelude::*;
use songbird::events::{Event, EventContext, EventHandler as SongbirdEventHandler, TrackEvent};
use songbird::input::YoutubeDl;
use std::sync::Arc;
use tokio::time::{Duration, sleep, timeout};

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
        handle
    } else {
        let join_result = timeout(Duration::from_secs(30), manager.join(guild_id, channel_id)).await;
        match join_result {
            Ok(Ok(h)) => h,
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

    let mut handler = handler_lock.lock().await;
    handler.stop();
    sleep(Duration::from_millis(350)).await;

    let source = YoutubeDl::new(HTTP_CLIENT.clone(), url.to_string()).user_args(vec![
        "--extractor-args".to_string(),
        "youtube:player_client=android,web".to_string(),
    ]);
    let track_handle = handler.play_input(source.into());

    let _ = track_handle.add_event(
        Event::Track(TrackEvent::Play),
        PlaybackNotifier {
            http: ctx.http.clone(),
            channel_id: msg.channel_id,
            text: "✅ Audio iniciado en el canal de voz.",
        },
    );

    let _ = track_handle.add_event(
        Event::Track(TrackEvent::Error),
        PlaybackNotifier {
            http: ctx.http.clone(),
            channel_id: msg.channel_id,
            text: "❌ La pista falló al reproducirse. Prueba otra URL; puede estar bloqueada para extracción.",
        },
    );

    let _ = track_handle.add_event(
        Event::Track(TrackEvent::End),
        PlaybackNotifier {
            http: ctx.http.clone(),
            channel_id: msg.channel_id,
            text: "ℹ️ La reproducción terminó.",
        },
    );

    let _ = msg
        .channel_id
        .say(&ctx.http, format!("🎵 Reproduciendo: {}", media_title))
        .await;
}

struct PlaybackNotifier {
    http: Arc<Http>,
    channel_id: ChannelId,
    text: &'static str,
}

#[async_trait]
impl SongbirdEventHandler for PlaybackNotifier {
    async fn act(&self, _ctx: &EventContext<'_>) -> Option<Event> {
        let _ = self.channel_id.say(&self.http, self.text).await;
        None
    }
}
