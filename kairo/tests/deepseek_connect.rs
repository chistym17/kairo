//! Connectivity smoke test for DeepSeekClient.
//!
//! Requires in `.env` (or the environment):
//! - `DEEPSEEK_API_KEY`
//! - optional `MODEL_ID` (defaults to `deepseek-v4-flash`)
//! - optional `DEEPSEEK_API_URL`
//!
//! Run:
//!   cargo test --test deepseek_connect -- --nocapture

use kairo::llm::deepseek_client::DeepSeekClient;

#[tokio::test]
async fn deepseek_client_connects() {
    dotenv::dotenv().ok();

    let api_key = match std::env::var("DEEPSEEK_API_KEY") {
        Ok(k) if !k.trim().is_empty() => k,
        _ => {
            eprintln!("SKIP: set DEEPSEEK_API_KEY to run DeepSeek connect test");
            return;
        }
    };

    let model = std::env::var("MODEL_ID").unwrap_or_else(|_| "deepseek-v4-flash".to_string());
    println!("[deepseek_connect] model={model}");

    let client = DeepSeekClient::new(api_key, model);
    let reply = client
        .ping()
        .await
        .unwrap_or_else(|e| panic!("DeepSeek connect failed: {e}"));

    println!("[deepseek_connect] reply={reply:?}");
    assert!(
        !reply.trim().is_empty(),
        "expected non-empty reply from DeepSeek"
    );
}
