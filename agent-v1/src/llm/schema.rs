use serde_json::{json, Value};

pub fn build_tools_schema() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read the contents of a file in the workspace. \
                    Always read a file before editing it so you know its current content. \
                    Path must be relative to the workspace root (e.g. 'calc.py', not '/abs/path/calc.py'). \
                    Returns the file contents as a string. \
                    If the file does not exist, returns an error — check the path and try again.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Relative path to the file from workspace root. \
                                Example: 'calc.py' or 'src/main.py'"
                        }
                    },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "Write new content to a file in the workspace, replacing its entire contents. \
                    Use this to fix bugs after you have read the file and identified what needs to change. \
                    You must provide the COMPLETE new file content — not just the changed lines. \
                    Path must be relative to the workspace root. \
                    After writing, always run the tests to verify your fix worked.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Relative path to the file from workspace root. \
                                Example: 'calc.py'"
                        },
                        "content": {
                            "type": "string",
                            "description": "The complete new content of the file. \
                                Must include all lines — not just the changed ones. \
                                Preserve indentation and formatting exactly."
                        }
                    },
                    "required": ["path", "content"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "run_command",
                "description": "Run a shell command in the workspace directory. \
                    Use this to run tests and verify your fixes. \
                    The working directory is always the workspace root. \
                    Allowed commands: python, python3, pytest, cargo, npm, node, cat, ls. \
                    Always run the tests after making a fix to confirm it worked. \
                    A non-zero exit code means the tests failed — read the output carefully.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "cmd": {
                            "type": "string",
                            "description": "The command to run. \
                                Examples: 'pytest test_calc.py', 'python3 -m pytest', 'cargo test'. \
                                Do not use shell operators like &&, ||, |, or >."
                        }
                    },
                    "required": ["cmd"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "finish",
                "description": "Signal that the task is complete. \
                    Only call this after you have seen the tests pass. \
                    Do NOT call this if tests are still failing. \
                    Do NOT call this before running the tests. \
                    If you cannot fix the problem after several attempts, \
                    call this with a summary explaining what you tried and why it failed.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "summary": {
                            "type": "string",
                            "description": "Brief description of what you did and the outcome. \
                                Example: 'Fixed the add function in calc.py — changed subtraction to addition. Tests now pass.'"
                        }
                    },
                    "required": ["summary"]
                }
            }
        }
    ])
}

pub fn parse_tool_call(
    tool_name: &str,
    args: &Value,
) -> Result<crate::state::step::Action, String> {
    use crate::state::step::Action;
    use std::path::PathBuf;

    match tool_name {
        "read_file" => {
            let path = args["path"]
                .as_str()
                .ok_or("read_file missing required arg: path")?;
            Ok(Action::ReadFile {
                path: PathBuf::from(path),
            })
        }

        "write_file" => {
            let path = args["path"]
                .as_str()
                .ok_or("write_file missing required arg: path")?;
            let content = args["content"]
                .as_str()
                .ok_or("write_file missing required arg: content")?;
            Ok(Action::WriteFile {
                path: PathBuf::from(path),
                content: content.to_string(),
            })
        }

        "run_command" => {
            let cmd = args["cmd"]
                .as_str()
                .ok_or("run_command missing required arg: cmd")?;
            Ok(Action::RunCommand {
                cmd: cmd.to_string(),
            })
        }

        "finish" => {
            let summary = args["summary"]
                .as_str()
                .ok_or("finish missing required arg: summary")?;
            Ok(Action::Finish {
                summary: summary.to_string(),
            })
        }

        unknown => Err(format!(
            "unknown tool name: '{}'. Valid tools: read_file, write_file, run_command, finish",
            unknown
        )),
    }
}
