//! Shared widgets ported from `frontend/src/components`.

pub mod armed_button;
pub mod decode_text;
pub mod project_pic;
pub mod ring;
pub mod settings_group;
pub mod toasts;

pub use armed_button::{ArmedButton, armed_button};
pub use decode_text::{DecodeText, decode_text};
pub use project_pic::{PicBox, ProjectPic, project_pic};
pub use ring::{Ring, ring};
pub use settings_group::{SettingsGroup, blurb_code, settings_group, settings_groups};
pub use toasts::{
    Toast, ToastAction, ToastAsk, ToastKind, ToastOptions, ToastStore, Toasts, push_toast,
    toast_action,
};
