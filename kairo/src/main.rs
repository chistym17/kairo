#[tokio::main]
async fn main() {
    let code = kairo::cli::main_from_env().await;
    std::process::exit(code);
}
