use std::{env, sync::Arc};

use breakwater_parser::SimpleFrameBuffer;
use clap::Parser;
use color_eyre::eyre::{self, Context};
use log::info;
use tokio::sync::{broadcast, mpsc};

use crate::{
    cli_args::CliArgs,
    prometheus_exporter::PrometheusExporter,
    server::Server,
    sinks::{DisplaySink, ffmpeg::FfmpegSink},
    statistics::{Statistics, StatisticsEvent, StatisticsInformationEvent, StatisticsSaveMode},
};

mod cli_args;
mod connection_buffer;
mod prometheus_exporter;
mod server;
mod sinks;
mod statistics;
#[cfg(test)]
mod test_helpers;

#[cfg(test)]
mod tests;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    color_eyre::install()?;

    // TODO: Is there a more nice way of doing this?
    if env::var("RUST_LOG").is_err() {
        unsafe { env::set_var("RUST_LOG", "info") }
    }
    env_logger::init();

    let args = CliArgs::parse();

    // Not using dynamic dispatch here for performance reasons
    let fb = Arc::new(SimpleFrameBuffer::new(args.width, args.height));

    // If we make the channel to big, stats will start to lag behind
    // TODO: Check performance impact in real-world scenario. Maybe the statistics thread blocks the other threads
    let (statistics_tx, statistics_rx) = mpsc::channel::<StatisticsEvent>(100);
    let (statistics_information_tx, statistics_information_rx) =
        broadcast::channel::<StatisticsInformationEvent>(2);
    let (terminate_signal_tx, terminate_signal_rx) = broadcast::channel::<()>(1);

    let statistics_save_mode = if args.disable_statistics_save_file {
        StatisticsSaveMode::Disabled
    } else {
        StatisticsSaveMode::Enabled {
            save_file: args.statistics_save_file.clone(),
            interval_s: args.statistics_save_interval_s,
        }
    };
    let mut statistics = Statistics::new(
        statistics_rx,
        statistics_information_tx,
        statistics_save_mode,
    );

    let mut server = Server::new(
        &args.listen_address,
        fb.clone(),
        statistics_tx.clone(),
        args.network_buffer_size
            .try_into()
            // This should never happen as clap checks the range for us
            .context(format!(
                "invalid network buffer size: {}",
                args.network_buffer_size
            ))?,
        args.connections_per_ip,
    )
    .await
    .context("unable to start pixelflut server")?;

    let mut prometheus_exporter = PrometheusExporter::new(
        &args.prometheus_listen_address,
        statistics_information_rx.resubscribe(),
    )
    .context("unable to start prometheus exporter")?;

    let server_listener_thread = tokio::spawn(async move { server.start().await });
    let statistics_thread = tokio::spawn(async move { statistics.run().await });
    let prometheus_exporter_thread = tokio::spawn(async move { prometheus_exporter.run().await });

    let mut display_sinks = Vec::<Box<dyn DisplaySink<SimpleFrameBuffer> + Send>>::new();

    #[cfg(all(feature = "native-display", not(feature = "egui")))]
    {
        use crate::sinks::native_display::NativeDisplaySink;

        if let Some(native_display_sink) = NativeDisplaySink::new(
            fb.clone(),
            &args,
            statistics_tx.clone(),
            statistics_information_rx.resubscribe(),
            terminate_signal_rx.resubscribe(),
        )
        .await
        .context("unable to start native display sink")?
        {
            display_sinks.push(Box::new(native_display_sink));
        }
    }

    #[cfg(feature = "vnc")]
    {
        use crate::sinks::vnc::VncSink;

        if let Some(vnc_sink) = VncSink::new(
            fb.clone(),
            &args,
            statistics_tx.clone(),
            statistics_information_rx.resubscribe(),
            terminate_signal_rx.resubscribe(),
        )
        .await
        .context("unable to start vnc sink")?
        {
            display_sinks.push(Box::new(vnc_sink));
        }
    }

    let mut ffmpeg_thread_present = false;
    if let Some(ffmpeg_sink) = FfmpegSink::new(
        fb.clone(),
        &args,
        statistics_tx.clone(),
        statistics_information_rx.resubscribe(),
        terminate_signal_rx.resubscribe(),
    )
    .await
    .context("unable to start ffmpeg sink")?
    {
        display_sinks.push(Box::new(ffmpeg_sink));
        ffmpeg_thread_present = true;
    }

    let mut sink_threads = Vec::new();
    for mut sink in display_sinks {
        sink_threads.push(tokio::spawn(async move {
            sink.run().await?;
            eyre::Result::<()>::Ok(())
        }));
    }

    #[cfg(feature = "egui")]
    {
        use sinks::egui::EguiSink;

        match EguiSink::new(
            fb.clone(),
            &args,
            statistics_tx.clone(),
            statistics_information_rx.resubscribe(),
            terminate_signal_rx.resubscribe(),
        )
        .await
        .context("unable to create egui sink")?
        {
            Some(mut egui_sink) => {
                tokio::spawn(handle_ctrl_c(terminate_signal_tx));

                // Some platforms require opening windows from the main thread.
                // The tokio::main macro uses Runtime::block_on(future) which runs the future on
                // the current thread, which should be the main thread right now.
                egui_sink.run().await.context("unable to run egui sink")?;
            }
            _ => {
                handle_ctrl_c(terminate_signal_tx).await?;
            }
        }
    }

    #[cfg(not(feature = "egui"))]
    handle_ctrl_c(terminate_signal_tx).await?;

    prometheus_exporter_thread.abort();
    server_listener_thread.abort();

    for sink_thread in sink_threads {
        sink_thread
            .await
            .context("unable to join sink thread")?
            .context("unable to stop sink")?;
    }

    // We need to stop this thread as the last, as others always try to send statistics to it
    statistics_thread.abort();

    if ffmpeg_thread_present {
        info!(
            "Successfully shut down (there might still be a ffmpeg process running - it's complicated)"
        );
    } else {
        info!("Successfully shut down");
    }

    Ok(())
}

async fn handle_ctrl_c(terminate_signal_tx: broadcast::Sender<()>) -> eyre::Result<()> {
    tokio::signal::ctrl_c()
        .await
        .context("unable to wait for ctrl + c")?;

    terminate_signal_tx
        .send(())
        .context("unable to signal termination")?;

    Ok(())
}
