//! OpenAI-compatible providers.

use gpui::TestAppContext;
use recur_domain::{CompatibleFlagDim, CompatibleFlagOption, CompatibleModel, CompatibleProvider};
use recur_services::fake::Overrides;

use super::harness::with;

fn dim(id: &str, label: &str, wire: &str) -> CompatibleFlagDim {
    let opt = |value: &str, label: &str, default: bool| CompatibleFlagOption {
        value: value.into(),
        label: label.into(),
        default,
    };
    CompatibleFlagDim {
        id: id.into(),
        label: label.into(),
        wire: wire.into(),
        options: vec![
            opt("low", "Low", false),
            opt("medium", "Medium", true),
            opt("high", "High", false),
        ],
    }
}

#[gpui::test]
fn picker_controls_presets_summarize_and_custom_opens_the_editor(cx: &mut TestAppContext) {
    let effort = dim("effort", "Effort", "reasoning.effort");
    let verbosity = dim("verbosity", "Verbosity", "text.verbosity");
    let preset = vec![effort.clone(), verbosity.clone()];
    let opts = Overrides::default().compatible_flag_preset(move |_, _| Ok(preset.clone()));
    let mut s = with(
        cx,
        |sd| {
            sd.settings.compatible_providers = vec![CompatibleProvider {
                id: "lmstudio".into(),
                name: "LM Studio".into(),
                base_url: "http://localhost:1234".into(),
                models: vec![CompatibleModel {
                    id: "qwen".into(),
                    label: "Qwen".into(),
                    flags: vec![effort, verbosity],
                    ..Default::default()
                }],
                ..Default::default()
            }];
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
