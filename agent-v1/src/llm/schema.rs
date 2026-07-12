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
                    Skips common heavy directories like node_modules and .git.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Relative directory (or file) path from workspace root. Example: '.' or 'src'"
                        }
                    },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "search_code",
                "description": "Search file contents for a text query (case-insensitive). \
                    Use this to locate symbols, components, or strings (e.g. 'Navbar', 'authMiddleware'). \
                    Returns matching lines as path:line: snippet. Skips binary files and heavy dirs.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Text to search for. Example: 'Navbar' or 'useAuth'"
                        },
                        "path": {
                            "type": "string",
                            "description": "Optional relative directory to scope the search. Default: '.'"
                        }
                    },
                    "required": ["query"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "find_files",
                "description": "Find files by glob pattern. \
                    Use this to discover file types or names (e.g. '*.tsx', '**/routes/*', '*test*'). \
                    Patterns without '/' also match basenames. Skips heavy dirs like node_modules.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Glob pattern. Examples: '*.tsx', '**/*.test.js', 'package.json'"
                        },
                        "path": {
                            "type": "string",
                            "description": "Optional relative directory to scope the search. Default: '.'"
                        }
                    },
                    "required": ["pattern"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "git_status",
                "description": "Show git status for the workspace (branch and changed files). \
                    Call this before editing to see existing modifications, and after edits to confirm what changed.",
                "parameters": {
                    "type": "object",
                    "properties": {}
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "git_diff",
                "description": "Show the working-tree git diff (vs HEAD). \
                    Use after edits to review your changes before finish. \
                    Optionally pass a path to scope the diff.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Optional relative file or directory to diff. Omit for full workspace diff."
                        }
                    }
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
                "description": "Run a shell command in the workspace. \
                    Use this to run tests, builds, and verify fixes. \
                    Optional working_directory scopes the command (e.g. 'apps/backend'). \
                    Optional timeout_secs raises the limit for slow builds (default 30, max 900). \
                    Returns structured output: exit_code, duration_ms, cwd, stdout, stderr. \
                    Allowed commands: python, python3, pytest, cargo, npm, npx, pnpm, yarn, node, cat, ls, make, jest. \
                    A non-zero exit_code means failure — read stderr carefully.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "cmd": {
                            "type": "string",
                            "description": "The command to run. \
                                Examples: 'npm test', 'cargo test', 'pytest'. \
                                Do not use shell operators like &&, ||, |, or >."
                        },
                        "working_directory": {
                            "type": "string",
                            "description": "Optional relative directory under the workspace. Default: '.' Example: 'apps/frontend'"
                        },
                        "timeout_secs": {
                            "type": "string",
                            "description": "Optional timeout in seconds as a string (default '30', max '900'). Example: '120'"
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
            Ok(Action::ListFiles {
                path: PathBuf::from(path),
            })
        }

        "search_code" => {
            let query = args["query"]
                .as_str()
                .ok_or("search_code missing required arg: query")?;
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");
            Ok(Action::SearchCode {
                query: query.to_string(),
                path: PathBuf::from(path),
            })
        }

        "find_files" => {
            let pattern = args["pattern"]
                .as_str()
                .ok_or("find_files missing required arg: pattern")?;
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");
            Ok(Action::FindFiles {
                pattern: pattern.to_string(),
                path: PathBuf::from(path),
            })
        }

        "git_status" => Ok(Action::GitStatus),

        "git_diff" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(PathBuf::from);
            Ok(Action::GitDiff { path })
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
            let cwd = args
                .get("working_directory")
                .or_else(|| args.get("cwd"))
                .and_then(|v| v.as_str())
                .unwrap_or(".");
            let timeout_secs = parse_timeout_secs(args.get("timeout_secs"))?;
            Ok(Action::RunCommand {
                cmd: cmd.to_string(),
                cwd: PathBuf::from(cwd),
                timeout_secs,
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
            "unknown tool name: '{}'. Valid tools: read_file, list_files, search_code, find_files, git_status, git_diff, write_file, apply_patch, run_command, finish",
            unknown
        )),
    }
}

fn parse_timeout_secs(value: Option<&Value>) -> Result<u64, String> {
    use crate::tools::run_command::{DEFAULT_TIMEOUT_SECS, MAX_TIMEOUT_SECS};

    let Some(v) = value else {
        return Ok(DEFAULT_TIMEOUT_SECS);
    };

    let parsed = if let Some(n) = v.as_u64() {
        n
    } else if let Some(n) = v.as_i64() {
        if n < 0 {
            return Err("run_command timeout_secs must be non-negative".to_string());
        }
        n as u64
    } else if let Some(s) = v.as_str() {
        let s = s.trim();
        if s.is_empty() {
            return Ok(DEFAULT_TIMEOUT_SECS);
        }
        s.parse::<u64>()
            .map_err(|_| format!("run_command timeout_secs must be an integer string, got {s:?}"))?
    } else if v.is_null() {
        return Ok(DEFAULT_TIMEOUT_SECS);
    } else {
        return Err("run_command timeout_secs must be a number or string".to_string());
    };

    if parsed == 0 {
        return Err("run_command timeout_secs must be >= 1".to_string());
    }
    Ok(parsed.min(MAX_TIMEOUT_SECS))
}
