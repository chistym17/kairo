#[tokio::main]
async fn main() {
    let code = agent_v1::cli::main_from_env().await;
    std::process::exit(code);
}
