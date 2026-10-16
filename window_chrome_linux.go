//go:build linux && cgo && !gtk3

package main

/*
#cgo pkg-config: gtk4
#include <gtk/gtk.h>

static void setPreferDark(int dark) {
	g_object_set(gtk_settings_get_default(), "gtk-application-prefer-dark-theme", dark ? TRUE : FALSE, NULL);
}
*/
import "C"

import "github.com/wailsapp/wails/v3/pkg/application"

// setNativeChromeDark makes GTK-drawn chrome (the menubar) match the app theme
// instead of the desktop one.
func setNativeChromeDark(dark bool) {
	d := C.int(0)
	if dark {
		d = 1
	}
	application.InvokeAsync(func() { C.setPreferDark(d) })
}
