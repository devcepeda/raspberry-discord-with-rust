use serenity::model::gateway::Ready;
use serenity::prelude::*;

pub async fn ready(_: Context, ready: Ready) {
    println!("{} está conectado!", ready.user.name);
}