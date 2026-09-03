mod config;

mod api;
mod message_handler;
mod proof_executor;
mod proof_store;
mod sp1_progress;
mod worker_state;

use crate::config::Config;
use crate::worker_state::WorkerState;
use anyhow::{Context, Result};
use clap::Parser;
use log::{debug, error, info, warn};
use sp1_sdk::utils;
use std::{net::SocketAddr, sync::Arc};

// used for dynamic environments that use multiple configurations, like running an integration test
// on a machine that has another config
#[derive(Parser)]
struct Args {
    /// Path to config file
    #[arg(short, long, default_value = "contemplant.toml")]
    config: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let config_file = Args::parse().config;
    debug!("Using config {config_file}");

    let config = Config::load(&config_file).context("load configuration")?;

    // Redirect our stderr through a tap BEFORE any prover (and thus the
    // sp1-gpu-server child, which inherits fd 2) is built, so the SP1
    // progress reader can see the server's lines. Best-effort: on failure
    // it leaves stderr untouched.
    sp1_progress::install();

    // Set up the SP1 SDK logger.
    utils::setup_logger();

    // Cap proving parallelism to leave scheduler headroom for the
    // async runtimes. risc0/SP1 CPU phases size their rayon pools from
    // RAYON_NUM_THREADS (default: every core); a fully saturated box
    // starves heartbeat/websocket threads long enough for hierophant
    // to evict a healthy, busy worker. Reserve two cores unless the
    // operator configured the knob explicitly.
    if std::env::var_os("RAYON_NUM_THREADS").is_none() {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let rayon_threads = cores.saturating_sub(2).max(1);
        // SAFETY: no prover or config-reading thread is running yet;
        // the runtime workers spawned by #[tokio::main] do not touch
        // the environment here.
        unsafe { std::env::set_var("RAYON_NUM_THREADS", rayon_threads.to_string()) };
        info!(
            "RAYON_NUM_THREADS unset; defaulting to {rayon_threads} of {cores} cores \
             (reserving headroom for liveness signaling)"
        );
    }

    info!("Starting contemplant {}", config.contemplant_name);

    let worker_state = WorkerState::new(config.clone()).await;

    // Create a broadcast channel for shutdown signal
    let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);
    let shutdown_tx_clone = shutdown_tx.clone();

    // Spawn a task to listen for SIGINT or SIGTERM and broadcast shutdown.
    // Handling SIGTERM gives us a chance to send a clean WebSocket Close
    // frame to hierophant on `docker stop` / `docker-compose down` before
    // docker's 10s grace period elapses and SIGKILL's us; otherwise
    // hierophant logs the TCP reset as an ERROR.
    tokio::spawn(async move {
        use tokio::signal::unix::{SignalKind, signal};
        let sigterm = signal(SignalKind::terminate());
        let mut sigterm = match sigterm {
            Ok(s) => s,
            Err(e) => {
                log::warn!("Failed to install SIGTERM handler: {e}. Only SIGINT will trigger graceful shutdown.");
                let _ = tokio::signal::ctrl_c().await;
                info!("Received SIGINT, stopping services...");
                let _ = shutdown_tx_clone.send(());
                return;
            }
        };
        tokio::select! {
            r = tokio::signal::ctrl_c() => {
                if let Err(e) = r {
                    log::warn!("SIGINT handler error: {e}");
                }
                info!("Received SIGINT, stopping services...");
            }
            _ = sigterm.recv() => {
                info!("Received SIGTERM, stopping services...");
            }
        }
        let _ = shutdown_tx_clone.send(());
    });

    // Messages to hierophant ride a channel that OUTLIVES any single
    // websocket connection: proving tasks hold the sender, and each
    // (re)connection's send loop drains the shared receiver. A proof
    // finishing during a reconnect window is delivered on the next
    // live connection instead of vanishing with the dead one.
    let (response_sender, response_receiver) = tokio::sync::mpsc::channel::<
        network_lib::messages::FromContemplantMessage,
    >(100);
    let response_receiver = Arc::new(tokio::sync::Mutex::new(response_receiver));

    // The hierophant connection lives on its OWN OS thread with a
    // dedicated single-thread runtime: liveness signaling must never
    // compete with the main runtime during proving. The loop
    // reconnects with backoff — previously a single websocket error
    // silenced the worker forever, and hierophant evicted it (and had
    // the Magister destroy it) one heartbeat window later.
    let worker_state_clone = worker_state.clone();
    let config_clone = config.clone();
    let conn_shutdown_tx = shutdown_tx.clone();
    let (conn_done_tx, hierophant_ws) = tokio::sync::oneshot::channel::<()>();
    std::thread::Builder::new()
        .name("hierophant-ws".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    error!("Failed to build hierophant-ws runtime: {e}");
                    let _ = conn_done_tx.send(());
                    return;
                }
            };
            rt.block_on(async move {
                let mut outer_shutdown_rx = conn_shutdown_tx.subscribe();
                let mut backoff_secs: u64 = 5;
                loop {
                    let connected_at = std::time::Instant::now();
                    if let Err(e) = api::connect_to_hierophant(
                        config_clone.clone(),
                        worker_state_clone.clone(),
                        response_sender.clone(),
                        response_receiver.clone(),
                        conn_shutdown_tx.subscribe(),
                    )
                    .await
                    .context("hierophant ws connection")
                    {
                        error!("Error in Hierophant connection channel: {e}");
                    }
                    // A broadcast on the shutdown channel means this was
                    // a deliberate stop, not a connection failure.
                    match outer_shutdown_rx.try_recv() {
                        Ok(())
                        | Err(tokio::sync::broadcast::error::TryRecvError::Closed) => {
                            break;
                        }
                        Err(_) => {}
                    }
                    // Capability demotions can empty the advertised VM set
                    // (e.g. an SP1-only worker whose CUDA backend
                    // permanently died). Re-registering with nothing to
                    // offer would keep the entry heartbeat-fresh forever on
                    // a useless rented instance; parking instead lets the
                    // hierophant's heartbeat eviction fire its magister
                    // drop endpoint and destroy this instance — the
                    // intended decommission path. (A plain process exit
                    // would be revived by supervisord seconds later with
                    // reset state, looping the death cycle indefinitely.)
                    if worker_state_clone.supported_vms().is_empty() {
                        error!(
                            "No proving capabilities remain; parking without \
                             reconnection so eviction can decommission this \
                             instance (manual action needed if no Magister \
                             manages it)"
                        );
                        break;
                    }
                    // A connection that survived a while earns a fresh
                    // backoff; rapid failures back off up to 60s.
                    if connected_at.elapsed() > std::time::Duration::from_secs(60) {
                        backoff_secs = 5;
                    }
                    warn!("Hierophant connection lost; reconnecting in {backoff_secs}s");
                    tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;
                    backoff_secs = (backoff_secs * 2).min(60);
                }
            });
            let _ = conn_done_tx.send(());
        })
        .context("spawn hierophant-ws thread")?;

    // Create the axum http router with all routes
    let app = api::create_router(Arc::new(worker_state.clone()));

    let http_addr: SocketAddr = ([0, 0, 0, 0], config.http_port).into();

    // Create shutdown signal handler for HTTP server
    let mut http_shutdown_rx = shutdown_tx.subscribe();
    let http_shutdown_signal = async move {
        let _ = http_shutdown_rx.recv().await;
    };

    // Run the HTTP server with graceful shutdown
    let http_server = tokio::spawn(async move {
        axum::serve(
            tokio::net::TcpListener::bind(http_addr)
                .await
                .context("bind http server to {http_addr}")
                .unwrap(),
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(http_shutdown_signal)
        .await
        .context("Axum serve on {http_addr}")
        .unwrap();
    });

    info!("Http server listening on {http_addr}");

    // Wait for both tasks
    tokio::select! {
        _ = hierophant_ws => info!("WebSocket connection with Hierophant has been terminated"),
        _ = http_server => info!("HTTP server shutdown complete"),
    }

    Ok(())
}
