# dmgbuild settings for build/darwin/package.sh: a 540x380 window with 96 px
# icons, the app at 28% and Applications at 72% of the width, centered just
# above the middle (Finder measures y from below its title bar).
# Defines: app (the .app path), background (.tiff), volume_icon (.icns).
import os.path

app = defines["app"]
w, h = 540, 380
y = h // 2 - 26

format = "UDZO"
files = [app]
symlinks = {"Applications": "/Applications"}
icon = defines["volume_icon"]
background = defines["background"]

window_rect = ((100, 100), (w, h))
default_view = "icon-view"
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
icon_size = 96
text_size = 12
grid_spacing = 100
icon_locations = {
    os.path.basename(app): (w * 28 // 100, y),
    "Applications": (w * 72 // 100, y),
}
