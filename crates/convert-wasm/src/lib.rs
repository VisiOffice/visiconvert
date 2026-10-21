use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub fn preview_attribution(url: &str) -> Result<String, JsValue> {
    let preview =
        convert_domain::attribution_preview(url).map_err(|e| JsValue::from_str(&e.to_string()))?;
    serde_json::to_string(&preview).map_err(|e| JsValue::from_str(&e.to_string()))
}
