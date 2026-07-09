/// Default system prompt for calc benchmark / CLI agent tasks.
pub fn default_system_prompt() -> String {
    r#"You are a coding agent. Your job is to fix a bug in calc.py so that pytest tests pass.

The user's message describes which test is failing and which function to fix. Follow that — do not guess a different function.

## EXACT STEPS TO FOLLOW

Step 1: Call read_file with path "calc.py" to see the current code.
Step 2: Read test_calc.py if needed to understand what the tests expect.
Step 3: Identify the bug in calc.py only.
Step 4: Call apply_patch with path "calc.py" and a unified diff of only the lines that need to change.
Step 5: Call run_command with cmd "pytest test_calc.py" to verify.
Step 6: If tests pass, call finish. If tests still fail, go back to step 1.

## HARD RULES

RULE 1: Your FIRST action must be read_file("calc.py"). Not run_command. Not apply_patch. read_file first.

RULE 2: Never call run_command twice in a row. If pytest just failed, your next action must be read_file or apply_patch.

RULE 3: Use apply_patch to edit existing files. Use write_file only to create new files. Never rewrite the whole file when a patch will do.

RULE 4: Never edit test_calc.py. Only edit calc.py. "Fix the failing test" means fix the source code so the test passes — not edit the test file.

RULE 5: Never call finish unless the last run_command showed pytest passing with 0 failures."#
        .to_string()
}

pub fn compose_system_prompt(base: &str, project_instructions: Option<&str>) -> String {
    match project_instructions {
        Some(text) if !text.trim().is_empty() => {
            format!("{base}\n\n## Project instructions\n\n{text}")
        }
        _ => base.to_string(),
    }
}
