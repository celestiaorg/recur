//go:build darwin && cgo

package code

/*
#cgo CFLAGS: -x objective-c -fobjc-arc -mmacosx-version-min=12.0
#cgo LDFLAGS: -framework AppKit -framework Foundation -framework CoreFoundation
#include <stdlib.h>
#import <AppKit/AppKit.h>
#import <CoreFoundation/CoreFoundation.h>

static CFDataRef CopyAppIconPNG(const char *path, int size) {
	@autoreleasepool {
		if (path == NULL || size <= 0) {
			return NULL;
		}
		NSString *p = [NSString stringWithUTF8String:path];
		if (p.length == 0) {
			return NULL;
		}
		NSImage *icon = [[NSWorkspace sharedWorkspace] iconForFile:p];
		if (icon == nil) {
			return NULL;
		}
		[icon setSize:NSMakeSize(size, size)];
		NSBitmapImageRep *rep = [[NSBitmapImageRep alloc]
			initWithBitmapDataPlanes:NULL
			pixelsWide:size
			pixelsHigh:size
			bitsPerSample:8
			samplesPerPixel:4
			hasAlpha:YES
			isPlanar:NO
			colorSpaceName:NSCalibratedRGBColorSpace
			bytesPerRow:0
			bitsPerPixel:0];
		if (rep == nil) {
			return NULL;
		}
		NSGraphicsContext *ctx = [NSGraphicsContext graphicsContextWithBitmapImageRep:rep];
		if (ctx == nil) {
			return NULL;
		}
		[NSGraphicsContext saveGraphicsState];
		[NSGraphicsContext setCurrentContext:ctx];
		[[NSColor clearColor] set];
		NSRectFill(NSMakeRect(0, 0, size, size));
		[icon drawInRect:NSMakeRect(0, 0, size, size)
			    fromRect:NSZeroRect
			   operation:NSCompositingOperationSourceOver
			    fraction:1.0
		      respectFlipped:YES
			       hints:nil];
		[NSGraphicsContext restoreGraphicsState];
		NSData *png = [rep representationUsingType:NSBitmapImageFileTypePNG properties:@{}];
		if (png == nil) {
			return NULL;
		}
		return (CFDataRef)CFBridgingRetain(png);
	}
}

static void ReleaseCFData(CFDataRef ref) {
	if (ref != NULL) {
		CFRelease(ref);
	}
}
*/
import "C"

import (
	"encoding/base64"
	"unsafe"
)

func appIconDataURL(appPath string) string {
	if appPath == "" {
		return ""
	}
	cpath := C.CString(appPath)
	defer C.free(unsafe.Pointer(cpath))
	data := C.CopyAppIconPNG(cpath, 64)
	if data == 0 {
		return ""
	}
	defer C.ReleaseCFData(data)
	n := int(C.CFDataGetLength(data))
	if n <= 0 {
		return ""
	}
	ptr := C.CFDataGetBytePtr(data)
	buf := C.GoBytes(unsafe.Pointer(ptr), C.int(n))
	return "data:image/png;base64," + base64.StdEncoding.EncodeToString(buf)
}
