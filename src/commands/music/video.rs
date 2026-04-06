use super::shared::{
    acquire_media_slot, cleanup_temp_dir_contents, command_failure_details, create_temp_dir,
    ensure_video_under_limit, ffmpeg_available, ffprobe_available,
    output_has_postprocessing_failure, purge_previous_temp_dirs, run_command_capture,
    select_final_media_file, send_file, simplify_yt_dlp_error, yt_dlp_available,
};
use serenity::model::channel::Message;
use serenity::prelude::*;
use std::path::Path;

pub(super) async fn download_video_and_send(ctx: &Context, msg: &Message, url: &str) {
    if !yt_dlp_available().await || !ffmpeg_available().await || !ffprobe_available().await {
        let _ = msg
            .channel_id
            .say(
                &ctx.http,
                "❌ Faltan dependencias (yt-dlp/ffmpeg/ffprobe). Instálalas para usar !ytdownload.",
            )
            .await;
        return;
    }

    let Some(_permit) = acquire_media_slot(ctx, msg).await else {
        return;
    };

    let _ = purge_previous_temp_dirs();

    let _ = msg
        .channel_id
        .say(
            &ctx.http,
            "⬇️ Descargando video... (límite final: 20MB, compresión automática)",
        )
        .await;

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

    let primary = run_video_download(url, &output_template, true).await;
    let output = match primary {
        Ok(out) if out.status.success() => out,
        Ok(out) => {
            let details = command_failure_details(&out);
            if output_has_postprocessing_failure(&out)
                || details.to_ascii_lowercase().contains("conversion failed")
            {
                // Clean partial files before retrying to reclaim disk space.
                let _ = cleanup_temp_dir_contents(temp_dir.path());
                let _ = msg
                    .channel_id
                    .say(
                        &ctx.http,
                        "ℹ️ Falló conversión automática del proveedor. Reintentando descarga en modo compatible...",
                    )
                    .await;

                match run_video_download(url, &output_template, false).await {
                    Ok(fallback) if fallback.status.success() => fallback,
                    Ok(fallback) => {
                        let fallback_details = command_failure_details(&fallback);
                        let _ = msg
                            .channel_id
                            .say(
                                &ctx.http,
                                format!(
                                    "❌ No pude descargar el video. {}",
                                    simplify_yt_dlp_error(&fallback_details)
                                ),
                            )
                            .await;
                        return;
                    }
                    Err(e) => {
                        let _ = msg
                            .channel_id
                            .say(&ctx.http, format!("❌ Falló yt-dlp: {}", e))
                            .await;
                        return;
                    }
                }
            } else {
                let _ = msg
                    .channel_id
                    .say(
                        &ctx.http,
                        format!(
                            "❌ No pude descargar el video. {}",
                            simplify_yt_dlp_error(&details)
                        ),
                    )
                    .await;
                return;
            }
        }
        Err(e) => {
            let _ = msg
                .channel_id
                .say(&ctx.http, format!("❌ Falló yt-dlp: {}", e))
                .await;
            return;
        }
    };

    if !output.status.success() {
        let details = command_failure_details(&output);
        let _ = msg
            .channel_id
            .say(
                &ctx.http,
                format!("❌ No pude descargar el video. {}", simplify_yt_dlp_error(&details)),
            )
            .await;
        return;
    }

    let Some(downloaded_video) =
        select_final_media_file(temp_dir.path(), &["mp4", "mkv", "webm", "mov", "m4v"])
    else {
        let _ = msg
            .channel_id
            .say(&ctx.http, "❌ Descarga incompleta: no encontré el video final.")
            .await;
        return;
    };

    let final_video = match ensure_video_under_limit(&downloaded_video, temp_dir.path()).await {
        Ok(path) => path,
        Err(e) => {
            let _ = msg.channel_id.say(&ctx.http, format!("❌ {}", e)).await;
            return;
        }
    };

    if send_file(ctx, msg, &final_video, "📦 Video solicitado")
        .await
        .is_err()
    {
        let _ = msg
            .channel_id
            .say(
                &ctx.http,
                "❌ No pude enviar el video. Verifica permisos de adjuntos en este canal.",
            )
            .await;
    } else {
        let sent_name = final_video
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("archivo.mp4");
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

async fn run_video_download(
    url: &str,
    output_template: &Path,
    force_mp4_postprocess: bool,
) -> Result<std::process::Output, String> {
    let mut args = vec![
        "--no-playlist".to_string(),
        "--no-warnings".to_string(),
        "--restrict-filenames".to_string(),
        "--concurrent-fragments".to_string(),
        "1".to_string(),
        "--max-filesize".to_string(),
        "80m".to_string(),
    ];

    let format = if force_mp4_postprocess {
        args.push("--merge-output-format".to_string());
        args.push("mp4".to_string());
        // Prefer H264+AAC: remux-only into mp4, no transcoding needed.
        // Falls back to pre-combined mp4, then any combined, then separate streams last resort.
        "bv[vcodec^=avc][ext=mp4]+ba[ext=m4a]/b[ext=mp4]/b/bv*+ba"
    } else {
        // Fallback mode: skip forced container, accept pre-combined or webm to avoid ffmpeg encode.
        "b[ext=mp4]/b[ext=webm]/b/bv*+ba"
    };

    args.push("-f".to_string());
    args.push(format.to_string());
    args.push("-o".to_string());
    args.push(output_template.to_string_lossy().to_string());
    args.push(url.to_string());

    run_command_capture("yt-dlp", args).await
}
