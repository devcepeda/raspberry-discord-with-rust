use serenity::model::channel::Message;
use serenity::prelude::*;

pub async fn run(ctx: &Context, msg: &Message) {

    if msg.content == "!ping" {
        let _ = msg.channel_id.say(&ctx.http, "Pong! 🏓").await;
    }

}