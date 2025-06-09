use network_lib::ProgressUpdate;

use crate::types::ProofStore;
use alloy_primitives::B256;
use anyhow::{Context, Result};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use indicatif_log_bridge::LogWrapper;
use lazy_static::lazy_static;
use log::{Log, Metadata, Record, info};
use sp1_sdk::CpuProver;
use sp1_sdk::{Prover, SP1Stdin};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, AsyncSeekExt, BufReader},
    sync::{mpsc, watch},
    time::{Duration, interval, sleep},
};

use crate::config::AssessorConfig;

// Custom logger wrapper
struct LogCapturingWrapper {
    buffer: Arc<Mutex<VecDeque<String>>>,
    capture_enabled: Arc<Mutex<bool>>,
}

impl LogCapturingWrapper {
    fn new() -> Self {
        Self {
            buffer: Arc::new(Mutex::new(VecDeque::new())),
            capture_enabled: Arc::new(Mutex::new(false)),
        }
    }

    fn enable_capture(&self) {
        *self.capture_enabled.lock().unwrap() = true;
    }

    fn disable_capture(&self) {
        *self.capture_enabled.lock().unwrap() = false;
    }

    fn get_captured_logs(&self) -> Vec<String> {
        let mut buffer = self.buffer.lock().unwrap();
        buffer.drain(..).collect()
    }
}

impl Log for LogCapturingWrapper {
    fn enabled(&self, metadata: &Metadata) -> bool {
        if *self.capture_enabled.lock().unwrap() {
            // When capturing, enable info+ logs regardless of RUST_LOG
            metadata.level() <= log::Level::Info
        } else {
            // When not capturing, respect env_logger settings
            let temp_logger = env_logger::Builder::from_default_env().build();
            temp_logger.enabled(metadata)
        }
    }

    fn log(&self, record: &Record) {
        // Always send to env_logger for normal console output (respects RUST_LOG)
        let temp_logger = env_logger::Builder::from_default_env().build();
        temp_logger.log(record);

        // Capture when enabled, regardless of RUST_LOG
        if *self.capture_enabled.lock().unwrap() && record.level() <= log::Level::Info {
            let formatted = format!("{} - {}", record.level(), record.args());
            self.buffer.lock().unwrap().push_back(formatted);
        }
    }

    fn flush(&self) {}
}

lazy_static! {
    static ref LOG_CAPTURER: LogCapturingWrapper = LogCapturingWrapper::new();
}

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

    // Initialize the logger with indicatif bridge
    log::set_boxed_logger(Box::new(LogWrapper::new(multi.clone(), &*LOG_CAPTURER)))?;
    log::set_max_level(log::LevelFilter::Info);

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
    LOG_CAPTURER.enable_capture();
    let (_, report) = mock_prover.execute(elf, sp1_stdin).run().unwrap();
    LOG_CAPTURER.disable_capture();
    let captured_logs = LOG_CAPTURER.get_captured_logs();
    let max_clk = find_max_clk_sync(captured_logs).unwrap();
    info!("... proof details {:?} ...", report);
    info!("... proof cycles {} ...", max_clk);
    proof_cycle_spinner.finish_with_message("... complete!");

    // Prepare a moongate watcher to estimate proof progress from logs.
    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
    let offset = tokio::fs::metadata(&config.moongate_log_path).await?.len();
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

// Function to find max clk value from logs
fn find_max_clk_sync(logs: Vec<String>) -> Option<u64> {
    let clk_regex = regex::Regex::new(r"clk\s*=\s*(\d+)").unwrap();
    let mut max_clk: Option<u64> = None;
    for line in logs {
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
    max_clk
}
