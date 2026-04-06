use super::shared::{
    acquire_media_slot, cleanup_temp_dir_contents, create_temp_dir, ensure_mp3_under_limit,
    ffmpeg_available, ffprobe_available, output_has_postprocessing_failure,
    purge_previous_temp_dirs, run_command_capture, select_final_media_file, send_file,
    simplify_yt_dlp_error, yt_dlp_available,
};
use serenity::model::channel::Message;
use serenity::prelude::*;

pub(super) async fn download_mp3_and_send(ctx: &Context, msg: &Message, url: &str) {
    if !yt_dlp_available().await || !ffmpeg_available().await || !ffprobe_available().await {
        let _ = msg
            .channel_id
            .say(
                &ctx.http,
                "❌ Faltan dependencias (yt-dlp/ffmpeg/ffprobe). Instálalas para usar !mp3.",
            )
            .await;
        return;
    }

    let Some(_permit) = acquire_media_slot(ctx, msg).await else {
        return;
    };

    let _ = purge_previous_temp_dirs();

    let _ = msg.channel_id.say(&ctx.http, "🎧 Preparando MP3...").await;

    let temp_dir = match create_temp_dir() {
        Ok(dir) => dir,
        Err(e) => {
            let _ = msg
                .channel_id
                .say(&ctx.http, format!("❌ No pude crear carpeta temporal: {}", e))
                .await;
            return;
        }
    };

    let output_template = temp_dir.path().join("%(title)s.%(ext)s");
    let output = run_command_capture(
        "yt-dlp",
        vec![
            "--no-playlist".to_string(),
            "--no-warnings".to_string(),
            "--restrict-filenames".to_string(),
            "--concurrent-fragments".to_string(),
            "1".to_string(),
            "--max-filesize".to_string(),
            "80m".to_string(),
            "-x".to_string(),
            "--audio-format".to_string(),
            "mp3".to_string(),
            "--audio-quality".to_string(),
            "0".to_string(),
            "-o".to_string(),
            output_template.to_string_lossy().to_string(),
            url.to_string(),
        ],
    )
    .await;

    let output = match output {
        Ok(out) => out,
        Err(e) => {
            let _ = msg
                .channel_id
                .say(&ctx.http, format!("❌ Falló yt-dlp: {}", e))
                .await;
            return;
        }
    };

    if !output.status.success() {
        let details = super::shared::command_failure_details(&output);
        eprintln!("yt-dlp mp3 falló para {}: {}", url, details);

        if output_has_postprocessing_failure(&output)
            || details.to_ascii_lowercase().contains("conversion failed")
        {
            let _ = msg
                .channel_id
                .say(
                    &ctx.http,
                    "ℹ️ Falló conversión a MP3. Reintentando en formato nativo...",
                )
                .await;
            let _ = cleanup_temp_dir_contents(temp_dir.path());

            let fallback = run_command_capture(
                "yt-dlp",
                vec![
                    "--no-playlist".to_string(),
                    "--no-warnings".to_string(),
                    "--restrict-filenames".to_string(),
                    "--concurrent-fragments".to_string(),
                    "1".to_string(),
                    "-x".to_string(),
                    "-f".to_string(),
                    "ba[ext=m4a]/ba[ext=opus]/ba/b".to_string(),
                    "-o".to_string(),
                    output_template.to_string_lossy().to_string(),
                    url.to_string(),
                ],
            )
            .await;

            match fallback {
                Ok(ref fb) if fb.status.success() => {}
                Ok(fb) => {
                    let fb_details = super::shared::command_failure_details(&fb);
                    let _ = msg
                        .channel_id
                        .say(
                            &ctx.http,
                            format!(
                                "❌ No pude generar audio nativo. {}",
                                simplify_yt_dlp_error(&fb_details)
                            ),
                        )
                        .await;
                    return;
                }
                Err(e) => {
                    let _ = msg
                        .channel_id
                        .say(&ctx.http, format!("❌ Falló yt-dlp (nativo): {}", e))
                        .await;
                    return;
                }
            }
        } else {
            let _ = msg
                .channel_id
                .say(
                    &ctx.http,
                    format!("❌ No pude generar MP3. {}", simplify_yt_dlp_error(&details)),
                )
                .await;
            return;
        }
    }

    let Some(downloaded_audio) =
        select_final_media_file(temp_dir.path(), &["mp3", "m4a", "webm", "ogg", "opus"])
    else {
        let _ = msg
            .channel_id
            .say(&ctx.http, "❌ Descarga incompleta: no encontré el MP3 final.")
            .await;
        return;
    };

    let final_audio = match ensure_mp3_under_limit(&downloaded_audio, temp_dir.path()).await {
        Ok(path) => path,
        Err(e) => {
            let _ = msg.channel_id.say(&ctx.http, format!("❌ {}", e)).await;
            return;
        }
    };

    if send_file(ctx, msg, &final_audio, "🎵 MP3 solicitado")
        .await
        .is_err()
    {
        let _ = msg
            .channel_id
            .say(
                &ctx.http,
                "❌ No pude enviar el MP3. Verifica permisos de adjuntos en este canal.",
            )
            .await;
    } else {
        let sent_name = final_audio
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("archivo.mp3");
        let _ = msg
            .channel_id
            .say(
                &ctx.http,
                format!("✅ Enviado: {}. Limpiando archivos temporales...", sent_name),
            )
            .await;
    }

    let _ = cleanup_temp_dir_contents(temp_dir.path());
}
