use network_lib::ProgressUpdate;

use crate::config::AssessorConfig;
use crate::types::ProofStore;
use alloy_primitives::B256;
use anyhow::{Context, Result};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use log::info;
use sp1_sdk::CpuProver;
use sp1_sdk::{Prover, SP1Stdin};
use std::sync::Arc;
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, AsyncSeekExt, BufReader},
    sync::{mpsc, watch},
    time::{Duration, interval, sleep},
};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{
    prelude::__tracing_subscriber_SubscriberExt,
    {EnvFilter, Registry},
};

const LOG_DIR: &str = "./execution-reports";

pub async fn start_assessor(
    mock_prover: Arc<CpuProver>,
    elf: &[u8],
    sp1_stdin: &SP1Stdin,
    config: AssessorConfig,
    shutdown_rx: watch::Receiver<bool>,
    proof_store: Arc<tokio::sync::Mutex<ProofStore>>,
    request_id: B256,
) -> Result<()> {
    // Create progress bar
    let multi = MultiProgress::new();

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
    let proof_cycle_spinner = ProgressBar::new_spinner()
        .with_message("Calculating proof cycles ...")
        .with_style(
            ProgressStyle::with_template("[{spinner:1.green}] [{elapsed_precise}] {msg}")
                .unwrap()
                .tick_strings(&["⢄", "⢂", "⢁", "⡁", "⡈", "⡐", "⡠"]),
        );
    proof_cycle_spinner.enable_steady_tick(Duration::from_millis(100));
    multi.add(proof_cycle_spinner.clone());
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
    proof_cycle_spinner.finish_with_message("... complete!");

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

    // Add a total estimated progress bar.
    let total_bar = ProgressBar::new(10_000)
        .with_message("Proving ...")
        .with_style(
            ProgressStyle::with_template(
                "[{elapsed_precise}] {msg:<16} {bar:<40.green} [~{eta_precise}]",
            )
            .unwrap()
            .progress_chars("=>-"),
        );
    total_bar.enable_steady_tick(Duration::from_millis(100));
    multi.add(total_bar.clone());

    // Add a proof execution progress bar.
    let execution_bar = ProgressBar::new(10_000)
        .with_message("Executing ...")
        .with_style(
            ProgressStyle::with_template(
                "[{elapsed_precise}] {msg:<16} {bar:<40.green} [~{eta_precise}]",
            )?
            .progress_chars("=>-"),
        );
    multi.add(execution_bar.clone());

    // Add a proof serialization progress bar.
    let mut serialization_bar_ref = None;

    // Allow proving to finish while handling progress updates.
    let mut progress_complete = false;
    let mut execution_progress = 0;
    let mut first_serialization = false;
    let mut serialization_progress = 0;
    tokio::spawn(async move {
        loop {
            tokio::select! {
                // Only poll progress channel if not complete.
                Some(update) = progress_rx.recv(), if !progress_complete => {
                    match update {
                        ProgressUpdate::Execution(p) => {
                            let execution_diff = p - execution_progress;
                            execution_progress = p.max(execution_progress);
                            // TODO: don't print this every time
                            info!("... execution {}%", execution_progress );
                            execution_bar.inc(execution_diff);
                            total_bar.inc((execution_diff as f64 * 0.6).floor() as u64);
                        },
                        ProgressUpdate::Serialization(p) => {
                            if !first_serialization {
                                let serialization_bar = ProgressBar::new(10_000)
                                    .with_message("Serializing ...")
                                    .with_style(
                                        ProgressStyle::with_template(
                                            "[{elapsed_precise}] {msg:<16} {bar:<40.green} [~{eta_precise}]",
                                        )
                                        .unwrap()
                                        .progress_chars("=>-"),
                                    );
                                multi.add(serialization_bar.clone());
                                serialization_bar_ref = Some(serialization_bar);
                            }
                            first_serialization = true;

                            let serialization_diff = p - serialization_progress;
                            serialization_progress = p.max(serialization_progress);
                            info!("... serialization {}%", serialization_progress);
                            if let Some(serialization_bar) = &serialization_bar_ref {
                                serialization_bar.inc(serialization_diff);
                                total_bar.inc((serialization_diff as f64 * 0.4).floor() as u64);
                            }
                        },
                        ProgressUpdate::Done => {
                            info!("proof done executing");

                            let serialization_diff = 100 - serialization_progress;
                            serialization_progress = 100;
                            info!("... serialization {}%", serialization_progress );
                            if let Some(serialization_bar) = &serialization_bar_ref {
                                serialization_bar.inc(serialization_diff);
                                total_bar.inc((serialization_diff as f64 * 0.4).floor() as u64);
                            }

                            progress_complete = true;
                        }
                    }

                    if let Some(proof_status) = proof_store.lock().await.get_mut(&request_id){
                        proof_status.progress_update(update)
                    }

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
    let max_clk = tokio::task::spawn_blocking(move || read_max_clk_from_file(&log_file_path))
        .await
        .context("Failed to read max clk from log file")??;

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
    let mut buffer = String::new();

    let mut final_clk_seen = false;
    let mut clk_shard_count = 0;
    let mut shard_count = 0;
    let mut last_clk = 0;

    let mut report_tick = interval(Duration::from_millis(config.watcher_reporting_interval_ms));

    loop {
        tokio::select! {
            _ = report_tick.tick() => {
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

            read = reader.read_line(&mut buffer) => {
                let bytes = read?;
                if bytes == 0 {
                    sleep(Duration::from_millis(config.watcher_polling_interval_ms)).await;
                    continue;
                }

                if let Some(line) = buffer.strip_suffix('\n') {
                    if !final_clk_seen && line.contains("clk =") {
                        if let Some(clk_val) = extract_clk(line) {
                            if clk_val > last_clk {
                                last_clk = clk_val;
                            }
                            if clk_val >= max_clk {
                                final_clk_seen = true;
                                last_clk = max_clk;
                                progress_tx.send(ProgressUpdate::Execution(100)).ok();
                            }
                        }
                    } else if !final_clk_seen && line.contains("Shard: [") {
                        clk_shard_count += 1;
                    } else if final_clk_seen && line.contains("Shard: [") {
                        shard_count += 1;
                    }
                }

                buffer.clear();
            }

            _ = shutdown_rx.changed() => {
                progress_tx.send(ProgressUpdate::Done).ok();
                return Ok(());
            }
        }
    }
}

fn extract_clk(line: &str) -> Option<u64> {
    line.split("clk =")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

fn read_max_clk_from_file(file_path: &str) -> Result<u64> {
    use std::fs::File;
    use std::io::{BufRead, BufReader};

    let file = File::open(file_path).context("Failed to open log file")?;
    let reader = BufReader::new(file);

    let clk_regex = regex::Regex::new(r"clk\s*=\s*(\d+)").unwrap();
    let mut max_clk: Option<u64> = None;

    for line in reader.lines() {
        let line = line.context("Failed to read line from log file")?;
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
