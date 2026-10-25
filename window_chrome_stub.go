//go:build !linux || !cgo || gtk3

package main

import "github.com/wailsapp/wails/v3/pkg/application"

func setNativeChromeDark(bool) {}

func setNativeWindowTitle(*application.WebviewWindow, string) {}
