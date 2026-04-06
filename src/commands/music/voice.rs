use serenity::model::channel::Message;
use serenity::prelude::*;

pub(super) async fn stop(ctx: &Context, msg: &Message) {
    let Some(guild_id) = msg.guild_id else {
        return;
    };

    let Some(manager) = songbird::get(ctx).await else {
        return;
    };

    if let Some(handler_lock) = manager.get(guild_id) {
        let mut handler = handler_lock.lock().await;
        handler.stop();
        let _ = msg
            .channel_id
            .say(&ctx.http, "⏹️ Reproducción detenida.")
            .await;
    } else {
        let _ = msg
            .channel_id
            .say(&ctx.http, "No estoy conectado a voz.")
            .await;
    }
}

pub(super) async fn leave(ctx: &Context, msg: &Message) {
    let Some(guild_id) = msg.guild_id else {
        return;
    };

    let Some(manager) = songbird::get(ctx).await else {
        return;
    };

    if manager.get(guild_id).is_some() {
        let _ = manager.remove(guild_id).await;
        let _ = msg
            .channel_id
            .say(&ctx.http, "👋 Salí del canal de voz.")
            .await;
    } else {
        let _ = msg
            .channel_id
            .say(&ctx.http, "No estoy conectado a voz.")
            .await;
    }
}
