//go:build !production || devtools

package main

// inspectorAvailable matches Wails: the inspector is compiled in for `wails3 task
// dev` (!production) and for packages built with RECUR_DEBUG=1 (devtools tag).
const inspectorAvailable = true
