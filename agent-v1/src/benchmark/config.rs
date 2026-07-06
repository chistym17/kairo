use std::path::PathBuf;

use crate::config::Config;

/// Benchmark-specific settings layered on top of the agent Config.
#[derive(Debug)]
pub struct BenchmarkConfig {
    pub agent: Config,
    pub tasks_path: PathBuf,
    pub trials: u32,
    pub output_dir: PathBuf,
    pub snapshots_dir: PathBuf,
}

impl BenchmarkConfig {
    pub fn load() -> Self {
        let agent = Config::load();

        Self {
            tasks_path: PathBuf::from(env_or("BENCHMARK_TASKS", "benchmarks/tasks.json")),
            trials: env_or("BENCHMARK_TRIALS", "1")
                .parse::<u32>()
                .expect("BENCHMARK_TRIALS must be a positive integer"),
            output_dir: PathBuf::from(env_or("BENCHMARK_OUTPUT", "benchmarks/results")),
            snapshots_dir: PathBuf::from(env_or(
                "BENCHMARK_SNAPSHOTS",
                "benchmarks/snapshots",
            )),
            agent,
        }
    }

    pub fn snapshot_for(&self, task_id: &str) -> PathBuf {
        self.snapshots_dir.join(task_id)
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}
