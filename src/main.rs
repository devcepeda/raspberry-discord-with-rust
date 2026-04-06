use serenity::{
    async_trait,
    model::{
        channel::Message,
        gateway::Ready,
    },
    prelude::*,
};

use songbird::SerenityInit;
use std::env;
use tokio::time::{Duration, sleep};

mod commands;
mod events;

struct Handler;

fn load_token() -> Result<String, String> {
    let token = env::var("DISCORD_TOKEN")
        .map_err(|_| "DISCORD_TOKEN no encontrado en el entorno o en .env".to_string())?;

    let trimmed = token.trim();
    if trimmed.is_empty() {
        return Err("DISCORD_TOKEN está vacío".to_string());
    }

    Ok(trimmed.to_string())
}

fn is_invalid_token_error(error: &serenity::Error) -> bool {
    matches!(
        error,
        serenity::Error::Http(http_error) if http_error.status_code().map(|status| status.as_u16()) == Some(401)
    )
}

#[async_trait]
impl EventHandler for Handler {

    async fn ready(&self, ctx: Context, ready: Ready) {
        events::ready::ready(ctx, ready).await;
    }

    async fn message(&self, ctx: Context, msg: Message) {
        if msg.author.bot {
            return;
        }

        let content = msg.content.trim();
        if !content.starts_with('!') {
            return;
        }

        println!("Mensaje recibido: {}", msg.content);

        commands::ping::run(&ctx, &msg).await;
        commands::music::run(&ctx, &msg).await;
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    dotenv::dotenv().ok();

    let token = match load_token() {
        Ok(token) => token,
        Err(error) => {
            eprintln!("Error de configuración: {}", error);
            std::process::exit(1);
        }
    };

    let intents =
        GatewayIntents::GUILDS
        | GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT
        | GatewayIntents::GUILD_VOICE_STATES;

    println!("Iniciando bot con reconexión automática...");

    loop {
        let mut client = match Client::builder(&token, intents)
            .event_handler(Handler)
            .register_songbird()
            .await
        {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Error creando cliente: {:?}", e);
                sleep(Duration::from_secs(5)).await;
                continue;
            }
        };

        if let Err(error) = client.start().await {
            eprintln!("Error del cliente: {:?}", error);

            if is_invalid_token_error(&error) {
                eprintln!("Token de Discord inválido o revocado. Corrige DISCORD_TOKEN antes de reintentar.");
                std::process::exit(1);
            }
        }

        eprintln!("Conexión cerrada. Reintentando en 5 segundos...");
        sleep(Duration::from_secs(5)).await;
    }
}