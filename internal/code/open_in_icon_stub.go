//go:build !darwin || !cgo

package code

func appIconDataURL(string) string { return "" }
