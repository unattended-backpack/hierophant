use network_lib::ProgressUpdate;

use crate::config::AssessorConfig;
use crate::proof_store::ProofStoreClient;
use alloy_primitives::B256;
use anyhow::{Context, Result};
use log::info;
use sp1_sdk::CpuProver;
use sp1_sdk::{Prover, SP1Stdin};
use std::sync::Arc;
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, AsyncSeekExt, BufReader},
    sync::{mpsc, watch},
    time::{Duration, interval},
};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{
    prelude::__tracing_subscriber_SubscriberExt,
    {EnvFilter, Registry},
};

const LOG_DIR: &str = "./execution-reports";

pub(super) async fn start_assessor(
    mock_prover: Arc<CpuProver>,
    elf: &[u8],
    sp1_stdin: &SP1Stdin,
    config: AssessorConfig,
    shutdown_rx: watch::Receiver<bool>,
    proof_store_client: ProofStoreClient,
    request_id: B256,
) -> Result<()> {
    info!("Starting assessor...");
    // Set up file appender for this specific proof execution
    let log_dir = std::path::Path::new(LOG_DIR);
    tokio::fs::create_dir_all(log_dir)
        .await
        .context("Failed to create logs directory")?;

    let file_name = format!("proof_execution_{}.log", request_id);
    let file_appender = tracing_appender::rolling::never(log_dir, &file_name);
    let (file_writer, _guard) = tracing_appender::non_blocking(file_appender);

    // Determine a cycle count.
    let _ = mock_prover.setup(elf);
    // Execute with file logging
    let (report, max_clk) = execution_report_with_file_logging(
        mock_prover.clone(),
        elf,
        sp1_stdin,
        file_writer,
        request_id,
        _guard,
    )
    .await?;

    info!("... proof details {:?} ...", report);
    info!("... proof cycles {} ...", max_clk);

    // Prepare a moongate watcher to estimate proof progress from logs.
    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
    let offset = tokio::fs::metadata(&config.moongate_log_path)
        .await
        .context(format!("Open moongate log at {}", config.moongate_log_path))?
        .len();
    tokio::spawn(follow_log_slice(
        config,
        offset,
        max_clk,
        shutdown_rx,
        progress_tx,
    ));

    // Allow proving to finish while handling progress updates.
    let mut progress_complete = false;
    tokio::spawn(async move {
        loop {
            tokio::select! {
                // Only poll progress channel if not complete.
                Some(update) = progress_rx.recv(), if !progress_complete => {
                    if let ProgressUpdate::Done = update {
                        progress_complete = true;
                    }

                    proof_store_client.proof_progress_update(request_id, update).await
                }

                else => {
                    break;
                }
            }
        }
    });

    Ok(())
}

async fn execution_report_with_file_logging<W>(
    mock_prover: Arc<CpuProver>,
    elf: &[u8],
    sp1_stdin: &SP1Stdin,
    file_writer: W,
    request_id: B256,
    _guard: WorkerGuard,
) -> Result<(sp1_sdk::ExecutionReport, u64)>
where
    W: for<'writer> tracing_subscriber::fmt::writer::MakeWriter<'writer> + Send + Sync + 'static,
{
    info!("Generating execution report...");
    // Create a temporary subscriber that writes to the file
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(file_writer)
        .with_ansi(false)
        .with_target(false)
        .with_thread_names(false);

    // Set up environment filter for the file logging
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"))
        .add_directive("hyper=off".parse().unwrap())
        .add_directive("p3_keccak_air=off".parse().unwrap())
        .add_directive("p3_fri=off".parse().unwrap())
        .add_directive("p3_dft=off".parse().unwrap())
        .add_directive("p3_challenger=off".parse().unwrap())
        .add_directive("sp1_cuda=info".parse().unwrap()); // Keep sp1 logs for clk parsing

    // Create a scoped subscriber for this execution
    let subscriber = Registry::default().with(env_filter).with(file_layer);

    // Execute within the tracing context
    let result =
        tracing::subscriber::with_default(subscriber, || mock_prover.execute(elf, sp1_stdin).run());

    let (_, report) = result.context("Failed to execute proof")?;

    // Read the log file to find max_clk
    let log_file_path = format!("{LOG_DIR}/proof_execution_{}.log", request_id);
    let max_clk = read_max_clk_from_file(&log_file_path)
        .await
        .context("Failed to read max clk from log file")?;

    // Keep the guard alive until we're done
    drop(_guard);

    Ok((report, max_clk))
}

pub async fn follow_log_slice(
    config: AssessorConfig,
    start_offset: u64,
    max_clk: u64,
    mut shutdown_rx: watch::Receiver<bool>,
    progress_tx: mpsc::UnboundedSender<ProgressUpdate>,
) -> Result<()> {
    let file = File::open(&config.moongate_log_path)
        .await
        .context(format!(
            "Open moongate log path {}",
            config.moongate_log_path
        ))?;
    let mut reader = BufReader::new(file);
    reader.seek(std::io::SeekFrom::Start(start_offset)).await?;

    let mut final_clk_seen = false;
    let mut clk_shard_count = 0;
    let mut shard_count = 0;
    let mut last_clk = 0;

    let mut poll_tick = interval(Duration::from_millis(config.watcher_polling_interval_ms));

    loop {
        tokio::select! {

            _ = poll_tick.tick() => {
                // Read all available lines in one batch
                let lines_processed = process_available_lines(
                    &mut reader,
                    &mut final_clk_seen,
                    &mut clk_shard_count,
                    &mut shard_count,
                    &mut last_clk,
                    max_clk,
                    &progress_tx,
                ).await?;

                // If no lines were read, the file hasn't grown yet
                if lines_processed == 0 {
                    continue;
                }

                if !final_clk_seen {
                    // 0 to 100
                    let progress = ((last_clk as f64 / max_clk as f64).min(1.0) * 100.0).round() as u64;
                    progress_tx.send(ProgressUpdate::Execution(progress)).ok();
                } else {
                    let est_total = (clk_shard_count as f64 * 2.2).max(1.0);
                    // 0 to 100
                    let progress = ((shard_count as f64 / est_total).min(1.0) * 100.0).round() as u64;
                    progress_tx.send(ProgressUpdate::Serialization(progress)).ok();
                }
            }


            _ = shutdown_rx.changed() => {
                return Ok(());
            }
        }
    }
}

async fn process_available_lines(
    reader: &mut BufReader<File>,
    final_clk_seen: &mut bool,
    clk_shard_count: &mut u64,
    shard_count: &mut u64,
    last_clk: &mut u64,
    max_clk: u64,
    progress_tx: &mpsc::UnboundedSender<ProgressUpdate>,
) -> Result<usize> {
    let mut buffer = String::new();
    let mut lines_processed = 0;

    loop {
        buffer.clear();
        let bytes = reader.read_line(&mut buffer).await?;

        // No more data available
        if bytes == 0 {
            break;
        }

        lines_processed += 1;

        if let Some(line) = buffer.strip_suffix('\n') {
            if !*final_clk_seen && line.contains("clk =") {
                if let Some(clk_val) = extract_clk(line) {
                    if clk_val > *last_clk {
                        *last_clk = clk_val;
                    }
                    if clk_val >= max_clk {
                        *final_clk_seen = true;
                        *last_clk = max_clk;
                        progress_tx.send(ProgressUpdate::Execution(100)).ok();
                    }
                }
            } else if !*final_clk_seen && line.contains("Shard: [") {
                *clk_shard_count += 1;
            } else if *final_clk_seen && line.contains("Shard: [") {
                *shard_count += 1;
            }
        }
    }

    Ok(lines_processed)
}

fn extract_clk(line: &str) -> Option<u64> {
    line.split("clk =")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

async fn read_max_clk_from_file(file_path: &str) -> Result<u64> {
    let file = File::open(file_path)
        .await
        .context("Failed to open log file")?;
    let reader = BufReader::new(file);
    let mut lines = reader.lines();

    let clk_regex = regex::Regex::new(r"clk\s*=\s*(\d+)").context("Create regex expression")?;
    let mut max_clk: Option<u64> = None;

    while let Some(line) = lines.next_line().await.context("next line")? {
        if let Some(captures) = clk_regex.captures(&line) {
            if let Some(number_str) = captures.get(1) {
                if let Ok(clk_value) = number_str.as_str().parse::<u64>() {
                    max_clk = match max_clk {
                        None => Some(clk_value),
                        Some(current_max) => Some(current_max.max(clk_value)),
                    };
                }
            }
        }
    }

    max_clk.ok_or_else(|| anyhow::anyhow!("No clk values found in log file"))
}
