use serde_json::Value;

use crate::llm::schema::build_tools_schema;

pub fn build_tools() -> Value {
    build_tools_schema()
}
