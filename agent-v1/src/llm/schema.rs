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
                "name": "list_files",
                "description": "List files under a directory in the workspace to avoid guessing paths. \
                    Use this when you are not sure where a file is located. \
                    Path must be relative to the workspace root (e.g. '.', 'src', 'tests'). \
                    Output is a newline-separated list of relative file paths. \
                    The listing is bounded by max_depth and max_entries and will skip common heavy directories like node_modules and .git.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Relative directory (or file) path from workspace root. Example: '.' or 'src'"
                        },
                        "max_depth": {
                            "type": "integer",
                            "description": "Maximum recursion depth. Default: 4",
                            "minimum": 1,
                            "maximum": 25
                        },
                        "max_entries": {
                            "type": "integer",
                            "description": "Maximum number of entries to return. Default: 500",
                            "minimum": 1,
                            "maximum": 10000
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
                "description": "Create a new file in the workspace. Only use this when the file does not exist yet. \
                    To edit an existing file, use apply_patch instead.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Relative path to the file from workspace root."
                        },
                        "content": {
                            "type": "string",
                            "description": "The complete content of the new file."
                        }
                    },
                    "required": ["path", "content"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "apply_patch",
                "description": "Apply a unified diff patch to an existing file in the workspace. \
                    Use this to fix bugs after reading the file. \
                    Only include the lines that change — do not rewrite the whole file. \
                    Path must be relative to the workspace root.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Relative path to the file from workspace root. Example: 'calc.py'"
                        },
                        "diff": {
                            "type": "string",
                            "description": "Unified diff patch. Example:\n--- calc.py\n+++ calc.py\n@@ -1,2 +1,2 @@\n def add(a, b):\n-    return a * b\n+    return a + b"
                        }
                    },
                    "required": ["path", "diff"]
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

        "list_files" => {
            let path = args["path"]
                .as_str()
                .ok_or("list_files missing required arg: path")?;
            let max_depth = args["max_depth"].as_u64().unwrap_or(4) as u32;
            let max_entries = args["max_entries"].as_u64().unwrap_or(500) as u32;
            Ok(Action::ListFiles {
                path: PathBuf::from(path),
                max_depth,
                max_entries,
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

        "apply_patch" => {
            let path = args["path"]
                .as_str()
                .ok_or("apply_patch missing required arg: path")?;
            let diff = args["diff"]
                .as_str()
                .ok_or("apply_patch missing required arg: diff")?;
            Ok(Action::ApplyPatch {
                path: PathBuf::from(path),
                diff: diff.to_string(),
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
            "unknown tool name: '{}'. Valid tools: read_file, list_files, write_file, apply_patch, run_command, finish",
            unknown
        )),
    }
}
