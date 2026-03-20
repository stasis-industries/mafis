use super::data::ExportSnapshot;

pub fn to_json(snapshot: &ExportSnapshot) -> Result<String, String> {
    serde_json::to_string_pretty(snapshot).map_err(|e| format!("JSON serialization failed: {e}"))
}
