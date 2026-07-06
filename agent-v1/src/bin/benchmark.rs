use agent_v1::benchmark::{self, BenchmarkConfig};

#[tokio::main]
async fn main() {
    dotenv::dotenv().ok();

    let config = BenchmarkConfig::load();

    if let Err(error) = benchmark::run_benchmark(config).await {
        eprintln!("[benchmark] error: {error:#}");
        std::process::exit(1);
    }
}
