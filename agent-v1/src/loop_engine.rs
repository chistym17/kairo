// pub async fn run(state:&mut AgentState,llm:&dyn LlmClinet,system_prompt:&str)->StopReason{

//     loop{
//         if state.step_count>=state.max_steps{
//             return StopReason::MaxStepsExceeded;
//         }

//         let action = match llm.next_action(system_prompt, &state.goal, &state.history).await {
//             Ok(a) => a,
//             Err(_) => return StopReason::ToolError,
//         };

//         let result=tools::dispatch(&action,&state.workspace_root);
//         let stop = result.stop_reason.clone();

//         let step = Step { ... };
//         logging::write_entry(&state.run_id, &state.model_id, &step);

//         state.history.push(step);
//         state.step_count += 1;

//         if let Some(reason) = stop {
//             return reason;
//         }

//     }
// }
