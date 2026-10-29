//! WebKit's macOS date picker (`WebDateTimePickerMac.mm`): a graphical `NSDatePicker` in a
//! borderless child window under the input, 139 by 148 points, on a backdrop with the
//! `controlBackgroundColor` fill and rounded corners except the one at the field.

use std::cell::Cell;

use gpui_kit::{Bounds, Pixels, Window};
use objc2::rc::Retained;
use objc2::runtime::NSObjectProtocol;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSBackingStoreType, NSBezierPath, NSColor, NSDatePicker, NSDatePickerElementFlags,
    NSDatePickerStyle, NSResponder, NSView, NSWindow, NSWindowOrderingMode, NSWindowStyleMask,
};
use objc2_foundation::{NSDateFormatter, NSLocale, NSPoint, NSRect, NSSize, NSString, NSTimeZone};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

const CALENDAR_WIDTH: f64 = 139.;
const CALENDAR_HEIGHT: f64 = 148.;
const CORNER_RADIUS: f64 = 10.;
const BORDER: f64 = 0.5;

define_class!(
    // SAFETY: NSView has no subclassing requirements and the class does not implement Drop.
    #[unsafe(super(NSView, NSResponder, objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RecurDatePickerBackdrop"]
    struct Backdrop;

    unsafe impl NSObjectProtocol for Backdrop {}

    impl Backdrop {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let f = self.frame();
            let (l, r) = (f.origin.x + BORDER, f.origin.x + f.size.width - BORDER);
            let (b, t) = (f.origin.y, f.origin.y + f.size.height);
            let path = NSBezierPath::new();
            let p = NSPoint::new;
            path.moveToPoint(p(l, t));
            path.lineToPoint(p(r - CORNER_RADIUS, t));
            path.curveToPoint_controlPoint1_controlPoint2(p(r, t - CORNER_RADIUS), p(r, t), p(r, t));
            path.lineToPoint(p(r, b + CORNER_RADIUS));
            path.curveToPoint_controlPoint1_controlPoint2(p(r - CORNER_RADIUS, b), p(r, b), p(r, b));
            path.lineToPoint(p(l + CORNER_RADIUS, b));
            path.curveToPoint_controlPoint1_controlPoint2(p(l, b + CORNER_RADIUS), p(l, b), p(l, b));
            path.lineToPoint(p(l, t));
            NSColor::controlBackgroundColor().setFill();
            path.fill();
        }
    }
);

pub struct EscapeIvars {
    escaped: Cell<bool>,
}

define_class!(
    // SAFETY: NSDatePicker has no subclassing requirements and the class does not implement
    // Drop.
    #[unsafe(super(NSDatePicker, objc2_app_kit::NSControl, NSView, NSResponder, objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RecurEscapeHandlingDatePicker"]
    #[ivars = EscapeIvars]
    struct EscapePicker;

    unsafe impl NSObjectProtocol for EscapePicker {}

    impl EscapePicker {
        /// `WKEscapeHandlingDatePicker`: Escape ends the picker.
        #[unsafe(method(cancelOperation:))]
        fn cancel_operation(&self, _sender: Option<&objc2::runtime::AnyObject>) {
            self.ivars().escaped.set(true);
        }
    }
);

/// An open picker. Dropping it closes the window.
pub struct Picker {
    window: Retained<NSWindow>,
    parent: Retained<NSWindow>,
    picker: Retained<EscapePicker>,
    view: Retained<NSView>,
    format: Retained<NSDateFormatter>,
}

fn content_view(window: &Window) -> Option<Retained<NSView>> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(h) = handle.as_raw() else {
        return None;
    };
    // SAFETY: AppKit hands out the window's content NSView; retain it for our use.
    unsafe { Retained::retain(h.ns_view.as_ptr().cast::<NSView>()) }
}

impl Picker {
    /// Opens under `anchor` (the input's border box, window coordinates) at `value`
    /// (`YYYY-MM-DD`, or empty for today). `keyboard` gives the calendar the keys, as a picker
    /// opened with Space does.
    pub fn open(
        anchor: Bounds<Pixels>,
        value: &str,
        dark: bool,
        keyboard: bool,
        window: &Window,
    ) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let view = content_view(window)?;
        let parent = view.window()?;
        let (x, y, h) = (
            f64::from(f32::from(anchor.origin.x)),
            f64::from(f32::from(anchor.origin.y)),
            f64::from(f32::from(anchor.size.height)),
        );
        let local = if view.isFlipped() {
            NSRect::new(NSPoint::new(x, y), NSSize::new(1., h))
        } else {
            let vh = view.frame().size.height;
            NSRect::new(NSPoint::new(x, vh - y - h), NSSize::new(1., h))
        };
        let on_screen = parent.convertRectToScreen(view.convertRect_toView(local, None));
        let rect = NSRect::new(
            NSPoint::new(on_screen.origin.x, on_screen.origin.y - CALENDAR_HEIGHT),
            NSSize::new(CALENDAR_WIDTH, CALENDAR_HEIGHT),
        );
        // SAFETY: the designated initializer with valid arguments, on the main thread.
        let win = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::ZERO,
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // SAFETY: plain property setters on a window we own.
        unsafe { win.setReleasedWhenClosed(false) };
        win.setHasShadow(true);
        win.setMovable(false);
        win.setBackgroundColor(Some(&NSColor::clearColor()));
        win.setOpaque(false);
        let backdrop: Retained<Backdrop> = {
            let this = Backdrop::alloc(mtm).set_ivars(());
            // SAFETY: NSView's designated initializer.
            unsafe { msg_send![super(this), initWithFrame: rect] }
        };
        win.setContentView(Some(&backdrop));
        win.setFrame_display(rect, true);
        // SAFETY: the appearance names are static AppKit strings.
        let name = unsafe {
            if dark {
                NSAppearanceNameDarkAqua
            } else {
                NSAppearanceNameAqua
            }
        };
        win.setAppearance(NSAppearance::appearanceNamed(name).as_deref());

        let utc = NSTimeZone::timeZoneWithName(&NSString::from_str("UTC"))?;
        let picker: Retained<EscapePicker> = {
            let this = EscapePicker::alloc(mtm).set_ivars(EscapeIvars {
                escaped: Cell::new(false),
            });
            let bounds = backdrop.bounds();
            // SAFETY: NSDatePicker's designated initializer.
            unsafe { msg_send![super(this), initWithFrame: bounds] }
        };
        picker.setBezeled(false);
        picker.setDrawsBackground(false);
        picker.setDatePickerStyle(NSDatePickerStyle::ClockAndCalendar);
        picker.setDatePickerElements(NSDatePickerElementFlags::YearMonthDay);
        picker.setTimeZone(Some(&utc));
        let format = NSDateFormatter::new();
        format.setLocale(Some(&NSLocale::localeWithLocaleIdentifier(
            &NSString::from_str("en_US_POSIX"),
        )));
        format.setTimeZone(Some(&utc));
        format.setDateFormat(Some(&NSString::from_str("yyyy-MM-dd")));
        backdrop.addSubview(&picker);
        // SAFETY: the parent is the key window this view lives in.
        unsafe { parent.addChildWindow_ordered(&win, NSWindowOrderingMode::Above) };
        let p = Picker {
            window: win,
            parent,
            picker,
            view,
            format,
        };
        p.set_value(value);
        if keyboard {
            p.parent.makeFirstResponder(Some(&p.picker));
        }
        Some(p)
    }

    /// Shows `value` (empty for today).
    pub fn set_value(&self, value: &str) {
        let today = chrono::Local::now()
            .date_naive()
            .format("%Y-%m-%d")
            .to_string();
        let v = if value.is_empty() { &today } else { value };
        if let Some(date) = self.format.dateFromString(&NSString::from_str(v)) {
            self.picker.setDateValue(&date);
        }
    }

    /// The picked date as `YYYY-MM-DD`.
    pub fn value(&self) -> String {
        self.format
            .stringFromDate(&self.picker.dateValue())
            .to_string()
    }

    /// Escape was pressed in the calendar.
    pub fn escaped(&self) -> bool {
        self.picker.ivars().escaped.get()
    }
}

impl Drop for Picker {
    fn drop(&mut self) {
        if self
            .parent
            .firstResponder()
            .is_some_and(|r| std::ptr::eq(&*r, &***self.picker as &NSResponder))
        {
            self.parent.makeFirstResponder(Some(&self.view));
        }
        self.picker.removeFromSuperviewWithoutNeedingDisplay();
        self.parent.removeChildWindow(&self.window);
        self.window.close();
    }
}
