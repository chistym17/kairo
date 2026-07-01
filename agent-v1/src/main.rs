mod llm;
mod logging;
mod loop_engine;
mod state;
mod tools;

fn main() {
    let _ = state::StopReason::TaskComplete;
    println!("compiles");
}
