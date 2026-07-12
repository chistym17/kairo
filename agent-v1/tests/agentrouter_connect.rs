//! Connectivity smoke test for AgentRouterClient.
//!
//! Requires in `.env` (or the environment):
//! - `AGENTROUTER_API_KEY`
//! - optional `MODEL_ID` (defaults to `glm-5.2`)
//! - optional `AGENTROUTER_BASE_URL` (defaults to `https://agentrouter.org/v1`)
//!
//! Run:
//!   cargo test --test agentrouter_connect -- --nocapture

use agent_v1::llm::agentrouter_client::{default_base_url, AgentRouterClient};

#[tokio::test]
async fn agentrouter_client_connects() {
    dotenv::dotenv().ok();

    let api_key = match std::env::var("AGENTROUTER_API_KEY") {
        Ok(k) if !k.trim().is_empty() && !k.contains("...") => k,
        _ => {
            eprintln!("SKIP: set AGENTROUTER_API_KEY to run AgentRouter connect test");
            return;
        }
    };

    let model = std::env::var("MODEL_ID").unwrap_or_else(|_| "glm-5.2".to_string());
    let base_url = std::env::var("AGENTROUTER_BASE_URL").unwrap_or_else(|_| default_base_url());
    println!("[agentrouter_connect] model={model} base_url={base_url}");

    let client = AgentRouterClient::new(api_key, model, base_url);
    let reply = client
        .ping()
        .await
        .unwrap_or_else(|e| panic!("AgentRouter connect failed: {e}"));

    println!("[agentrouter_connect] reply={reply:?}");
    assert!(
        !reply.trim().is_empty(),
        "expected non-empty reply from AgentRouter"
    );
}
