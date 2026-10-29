//! Shared widgets.

pub mod armed_button;
pub mod calendar;
pub mod color_chooser;
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod date_picker_mac;
pub mod date_time;
pub mod decode_text;
pub mod icon_picker;
pub mod project_pic;
pub mod resize_handle;
pub mod ring;
pub mod scrollbar;
pub mod select;
pub mod settings_group;
pub mod tab_chip;
pub mod tab_strip;
pub mod toasts;

pub use armed_button::{ArmedButton, armed_button};
pub use date_time::{DateTimeEvent, DateTimeField, DateTimeStyle};
pub use decode_text::{DecodeText, decode_text};
pub use icon_picker::{IconPicked, IconPicker};
pub use project_pic::{PicBox, ProjectPic, project_pic};
pub use resize_handle::{Axis, Edge, ResizeHandle, resize_handle};
pub use ring::{Ring, ring};
pub use scrollbar::{ScrollArea, scroll_area};
pub use select::{Select, SelectEvent, SelectOption, SelectSize, TriggerStyle};
pub use settings_group::{SettingsGroup, blurb_code, settings_group, settings_groups};
pub use tab_chip::{ChipIcon, TabChip, tab_chip};
pub use tab_strip::{TabItem, TabStrip, tab_strip, tab_strip_action};
pub use toasts::{
    Toast, ToastAction, ToastAsk, ToastKind, ToastOptions, ToastStore, Toasts, push_toast,
    toast_action,
};
