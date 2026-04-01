use wasm_bindgen::prelude::*;

/// Parse a TJSON string and return it as a JSON string.
#[wasm_bindgen]
pub fn parse(input: &str) -> Result<String, String> {
    let json: serde_json::Value = crate::from_str(input).map_err(|e| e.to_string())?;
    serde_json::to_string(&json).map_err(|e| e.to_string())
}

/// Render a JSON string as TJSON, with optional options object.
#[wasm_bindgen]
pub fn stringify(input: &str, options: JsValue) -> Result<String, String> {
    let json: serde_json::Value = serde_json::from_str(input).map_err(|e| e.to_string())?;
    let opts = if options.is_null() || options.is_undefined() {
        crate::TjsonOptions::default()
    } else {
        let config: crate::TjsonConfig = serde_wasm_bindgen::from_value(options)
            .map_err(|e| e.to_string())?;
        config.into()
    };
    crate::to_string_with(&json, opts).map_err(|e| e.to_string())
}
