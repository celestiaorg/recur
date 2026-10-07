//go:build darwin && cgo

package main

/*
#cgo CFLAGS: -x objective-c -fobjc-arc -mmacosx-version-min=12.0
#cgo LDFLAGS: -framework Foundation
#include <stdlib.h>
#import <Foundation/Foundation.h>

char *copyAppleActionOnDoubleClick(void) {
	NSString *v = [[NSUserDefaults standardUserDefaults] stringForKey:@"AppleActionOnDoubleClick"];
	if (v == nil) {
		return NULL;
	}
	return strdup([v UTF8String]);
}
*/
import "C"

import "unsafe"

func currentTitlebarDoubleClickPref() string {
	p := C.copyAppleActionOnDoubleClick()
	if p == nil {
		return ""
	}
	defer C.free(unsafe.Pointer(p))
	return C.GoString(p)
}
