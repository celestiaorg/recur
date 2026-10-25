//go:build linux && cgo && !gtk3

package main

/*
#cgo pkg-config: gtk4
#include <stdlib.h>
#include <gtk/gtk.h>

static void setPreferDark(int dark) {
	g_object_set(gtk_settings_get_default(), "gtk-application-prefer-dark-theme", dark ? TRUE : FALSE, NULL);
}
*/
import "C"

import (
	"unsafe"

	"github.com/wailsapp/wails/v3/pkg/application"
	"github.com/wailsapp/wails/v3/pkg/events"
)

// setNativeChromeDark makes GTK-drawn chrome (the menubar) match the app theme
// instead of the desktop one.
func setNativeChromeDark(dark bool) {
	d := C.int(0)
	if dark {
		d = 1
	}
	application.InvokeAsync(func() { C.setPreferDark(d) })
}

// setNativeWindowTitle titles the GTK window once it is shown. Wails skips
// gtk_window_set_title for frameless windows, which would leave the taskbar and
// alt-tab showing the binary name instead of the window title.
func setNativeWindowTitle(win *application.WebviewWindow, title string) {
	win.OnWindowEvent(events.Common.WindowShow, func(*application.WindowEvent) {
		gtkWin := win.NativeWindow()
		if gtkWin == nil {
			return
		}
		application.InvokeAsync(func() {
			cTitle := C.CString(title)
			defer C.free(unsafe.Pointer(cTitle))
			C.gtk_window_set_title((*C.GtkWindow)(gtkWin), cTitle)
		})
	})
}
