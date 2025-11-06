 //! Session setup and initialization
use crate::analytics::set_wallet_address_for_reporting;
use crate::config::Config;
use crate::environment::Environment;
use crate::events::Event;
use crate::orchestrator::OrchestratorClient;
use crate::runtime::start_authenticated_worker;
use ed25519_dalek::SigningKey;
use std::error::Error;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

/// Session data for both TUI and headless modes
#[derive(Debug)]
pub struct SessionData {
    pub event_receiver: mpsc::Receiver<Event>,
    pub join_handles: Vec<JoinHandle<()>>,
    pub shutdown_sender: broadcast::Sender<()>,
    pub max_tasks_shutdown_sender: broadcast::Sender<()>,
    pub node_id: u64,
    pub orchestrator: OrchestratorClient,
    pub num_workers: usize,
}

// ----------------------------------------------------------------------
// 🧮 MEMORY AND THREAD CONTROL
// ----------------------------------------------------------------------

// Mỗi thread dự kiến dùng ~1.5 GB
const MEMORY_PER_THREAD: u64 = (1.5 * 1024.0 * 1024.0 * 1024.0) as u64; // bytes

/// Clamp số thread theo RAM khả dụng
fn clamp_threads_by_memory(requested_threads: usize) -> usize {
    let mut sys = System::new();
    sys.refresh_memory();
    let total_bytes = sys.total_memory() * 1024; // KB → bytes
    let max_threads_by_memory = (total_bytes / MEMORY_PER_THREAD) as usize;

    requested_threads.min(max_threads_by_memory.max(1))
}

/// Cảnh báo OOM nếu RAM không đủ
pub fn warn_memory_configuration(max_threads: Option<u32>) {
    if let Some(threads) = max_threads {
        let current_pid = Pid::from(std::process::id() as usize);

        let mut sysinfo = System::new();
        sysinfo.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[current_pid]),
            true,
            ProcessRefreshKind::nothing().with_memory(),
        );

        if let Some(process) = sysinfo.process(current_pid) {
            let ram_total = process.memory() * 1024; // KB → bytes
            if threads as u64 * MEMORY_PER_THREAD >= ram_total {
                crate::print_cmd_warn!(
                    "⚠️ OOM warning",
                    "Projected memory usage across {} threads (~1.5GB each) may exceed available memory.",
                    threads
                );
                std::thread::sleep(std::time::Duration::from_secs(3));
            }
        }
    }
}

// ----------------------------------------------------------------------
// 🚀 SESSION SETUP
// ----------------------------------------------------------------------

pub async fn setup_session(
    config: Config,
    env: Environment,
    check_mem: bool,
    max_threads: Option<u32>,
    max_tasks: Option<u32>,
    max_difficulty: Option<crate::nexus_orchestrator::TaskDifficulty>,
) -> Result<SessionData, Box<dyn Error>> {
    let node_id = config.node_id.parse::<u64>()?;
    let client_id = config.user_id;

    // Signing key
    let mut csprng = rand_core::OsRng;
    let signing_key: SigningKey = SigningKey::generate(&mut csprng);

    // Orchestrator client
    let orchestrator_client = OrchestratorClient::new(env.clone());

    // Số worker theo max_threads hoặc RAM
    let requested_threads = max_threads.unwrap_or(40) as usize;
    let mut num_workers = clamp_threads_by_memory(requested_threads);

    if check_mem {
        warn_memory_configuration(Some(num_workers as u32));
    }

    // Shutdown channel
    let (shutdown_sender, _) = broadcast::channel(1);

    // Set wallet
    set_wallet_address_for_reporting(config.wallet_address.clone());

    // Start authenticated workers
    let (event_receiver, join_handles, max_tasks_shutdown_sender) = start_authenticated_worker(
        node_id,
        signing_key,
        orchestrator_client.clone(),
        shutdown_sender.subscribe(),
        env,
        client_id,
        max_tasks,
        max_difficulty,
        num_workers,
    )
    .await;

    println!(
        "✅ Session started: {} workers active (~{} GB estimated RAM)",
        num_workers,
        (num_workers as f64 * 1.5).ceil()
    );

    Ok(SessionData {
        event_receiver,
        join_handles,
        shutdown_sender,
        max_tasks_shutdown_sender,
        node_id,
        orchestrator: orchestrator_client,
        num_workers,
    })
}

