use serde_json::{json, Value};

/// Build the tools array for the Anthropic Messages API
pub fn coding_tools() -> Vec<Value> {
    vec![
        json!({
            "name": "read_file",
            "description": "Read the contents of a file at the given path. Returns the file content as a string.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The absolute or relative file path to read"
                    }
                },
                "required": ["path"]
            }
        }),
        json!({
            "name": "write_file",
            "description": "Write content to a file, creating it if it doesn't exist or overwriting if it does.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The file path to write to"
                    },
                    "content": {
                        "type": "string",
                        "description": "The content to write to the file"
                    }
                },
                "required": ["path", "content"]
            }
        }),
        json!({
            "name": "edit_file",
            "description": "Edit a file by replacing an exact string match with new content.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The file path to edit"
                    },
                    "old_string": {
                        "type": "string",
                        "description": "The exact string to find and replace (must be unique in the file)"
                    },
                    "new_string": {
                        "type": "string",
                        "description": "The replacement string"
                    }
                },
                "required": ["path", "old_string", "new_string"]
            }
        }),
        json!({
            "name": "bash",
            "description": "Execute a bash command and return stdout/stderr. Use for running tests, git, npm, etc.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The bash command to execute"
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "description": "Timeout in milliseconds (default: 120000)"
                    }
                },
                "required": ["command"]
            }
        }),
        json!({
            "name": "list_directory",
            "description": "List files and directories at the given path.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The directory path to list"
                    }
                },
                "required": ["path"]
            }
        }),
        json!({
            "name": "search_files",
            "description": "Search for files matching a pattern or search within file contents using grep.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "The search pattern (regex for content search, glob for file search)"
                    },
                    "path": {
                        "type": "string",
                        "description": "The directory to search in"
                    },
                    "content_search": {
                        "type": "boolean",
                        "description": "If true, search file contents; if false, search file names"
                    }
                },
                "required": ["pattern", "path"]
            }
        }),
    ]
}

/// Determine if a tool is read-only (safe to auto-approve in auto_read mode)
pub fn is_read_only(tool_name: &str) -> bool {
    matches!(tool_name, "read_file" | "list_directory" | "search_files")
}
