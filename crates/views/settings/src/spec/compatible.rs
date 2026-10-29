//! `compatible-provider.spec.ts`.

use gpui::TestAppContext;
use recur_services::mock::{MockOptions, Reply};
use serde_json::{Value, json};

use super::harness::with;

fn dim(id: &str, label: &str, wire: &str) -> Value {
    json!({ "id": id, "label": label, "wire": wire, "options": [
        { "value": "low", "label": "Low" },
        { "value": "medium", "label": "Medium", "default": true },
        { "value": "high", "label": "High" },
    ] })
}

#[gpui::test]
fn picker_controls_presets_summarize_and_custom_opens_the_editor(cx: &mut TestAppContext) {
    let effort = dim("effort", "Effort", "reasoning.effort");
    let verbosity = dim("verbosity", "Verbosity", "text.verbosity");
    let preset = json!([effort.clone(), verbosity.clone()]);
    let opts = MockOptions::default().resolver("CompatibleFlagPreset", move |_, _, _| {
        Reply::value(preset.clone())
    });
    let mut s = with(
        cx,
        |sd| {
            sd.settings["compatibleProviders"] = json!([{
                "id": "lmstudio", "name": "LM Studio", "baseUrl": "http://localhost:1234",
                "models": [{ "id": "qwen", "label": "Qwen", "flags": [effort, verbosity] }],
            }]);
        },
        opts,
    );
    s.open_settings();
    s.section("Providers");
    assert!(s.until(|s| s.has("sel:Picker controls=Effort, verbosity")));
    assert!(s.has("compat-flag-opts:Low, Medium (default), High"));
    for c in 0..4 {
        let head = s.bounds(&format!("compat-cols:{c}")).expect("head");
        let cell = s.bounds(&format!("compat-model:0:{c}")).expect("cell");
        assert!(
            (f32::from(head.origin.x) - f32::from(cell.origin.x)).abs() < 1.,
            "column {c} header aligns with its field"
        );
    }
    assert!(!s.has("compat-flag-card:0"));

    s.pick("Picker controls", "Effort, verbosity", "Custom…");
    assert!(s.has("sel:Picker controls=Custom…"));
    assert!(s.has("compat-flag-card:0"));
    assert!(s.has("compat-flag-card:1"));
    assert!(!s.has("compat-flag-card:2"));
    assert!(!s.has("compat-flag-opts:Low, Medium (default), High"));

    assert!(s.has("compat-default:0:Default Medium:true"));
    s.click("compat-default:0:Default High:false");
    assert!(s.has("compat-default:0:Default High:true"));
    assert!(s.has("compat-default:0:Default Medium:false"));

    s.pick("Picker controls", "Custom…", "Effort, verbosity");
    assert!(!s.has("compat-flag-card:0"));
}
