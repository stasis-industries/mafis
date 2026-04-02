#[cfg(not(target_arch = "wasm32"))]
pub fn output_file(filename: &str, content: &str) -> Result<(), String> {
    use std::fs;
    use std::path::Path;

    let dir = Path::new("exports");
    if !dir.exists() {
        fs::create_dir_all(dir).map_err(|e| format!("Failed to create exports dir: {e}"))?;
    }

    let path = dir.join(filename);
    fs::write(&path, content).map_err(|e| format!("Failed to write {}: {e}", path.display()))?;

    bevy::log::info!("Exported: {}", path.display());
    Ok(())
}

#[cfg(target_arch = "wasm32")]
pub fn output_file(filename: &str, content: &str) -> Result<(), String> {
    use js_sys::Array;
    use wasm_bindgen::JsCast;
    use web_sys::{Blob, BlobPropertyBag, Url};

    let window = web_sys::window().ok_or("No window object")?;
    let document = window.document().ok_or("No document object")?;

    let js_content = wasm_bindgen::JsValue::from_str(content);
    let array = Array::new();
    array.push(&js_content);

    let opts = BlobPropertyBag::new();
    opts.set_type("text/plain;charset=utf-8");

    let blob = Blob::new_with_str_sequence_and_options(&array, &opts)
        .map_err(|_| "Failed to create Blob".to_string())?;

    let url = Url::create_object_url_with_blob(&blob)
        .map_err(|_| "Failed to create object URL".to_string())?;

    let anchor = document
        .create_element("a")
        .map_err(|_| "Failed to create anchor".to_string())?
        .dyn_into::<web_sys::HtmlAnchorElement>()
        .map_err(|_| "Failed to cast to HtmlAnchorElement".to_string())?;

    anchor.set_href(&url);
    anchor.set_download(filename);
    let _ = anchor.style().set_property("display", "none");

    let body = document.body().ok_or("No body element")?;
    body.append_child(&anchor).map_err(|_| "Failed to append anchor".to_string())?;
    anchor.click();
    let _ = body.remove_child(&anchor);
    let _ = Url::revoke_object_url(&url);

    bevy::log::info!("Browser download triggered: {filename}");
    Ok(())
}
