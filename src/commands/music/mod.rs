use serenity::model::channel::Message;
use serenity::prelude::*;

mod mp3;
mod play;
mod shared;
mod video;
mod voice;

pub async fn run(ctx: &Context, msg: &Message) {
    if !msg.content.starts_with('!') {
        return;
    }

    if msg.content == "!stop" {
        voice::stop(ctx, msg).await;
        return;
    }

    if msg.content == "!leave" {
        voice::leave(ctx, msg).await;
        return;
    }

    if let Some(url) = parse_arg(&msg.content, "!mp3") {
        mp3::download_mp3_and_send(ctx, msg, url).await;
        return;
    }

    if let Some(url) = parse_arg(&msg.content, "!ytdownload") {
        video::download_video_and_send(ctx, msg, url).await;
        return;
    }

    if let Some(url) = parse_arg(&msg.content, "!play") {
        play::play_url(ctx, msg, url).await;
        return;
    }

    if let Some(url) = parse_arg(&msg.content, "!pplay") {
        play::play_url(ctx, msg, url).await;
        return;
    }

    if let Some(url) = parse_arg(&msg.content, "!yt") {
        play::play_url(ctx, msg, url).await;
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
