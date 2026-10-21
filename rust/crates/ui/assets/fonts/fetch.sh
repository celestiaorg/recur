#!/usr/bin/env bash
# Rebuilds the bundled UI fonts from the Google Fonts request in frontend/index.html.
#
# GPUI selects faces by family + weight and cannot pick a wght inside a variable font,
# so every weight the web UI requests is bundled as a static TTF. Google serves those
# statics to non-woff2 clients; they are instanced from the same variable masters the
# browser renders. Inter 450 has no Google static, so it is instanced here from the
# google/fonts variable source (opsz pinned to its default 14, as Google does for
# wght-only requests). The instancer reproduces Google's Inter 400 static exactly.
#
# Requires curl and uv (fontTools runs via `uv run`).
set -euo pipefail
cd "$(dirname "$0")"

query='family=Space+Grotesk:wght@500;600;700&family=Inter:wght@400;450;500;600&family=JetBrains+Mono:wght@400;500;600;700&family=IBM+Plex+Mono:wght@400;500;600;700&family=Source+Code+Pro:wght@400;500;600;700'
curl -fsS "https://fonts.googleapis.com/css2?$query" |
  tr -d '\n' | grep -oE "font-family: '[^']+';[^}]*font-weight: [0-9]+;[^}]*url\([^)]+\)" |
  sed -E "s/font-family: '([^']+)';.*font-weight: ([0-9]+);.*url\(([^)]+)\)/\1|\2|\3/" |
  while IFS='|' read -r family weight url; do
    curl -fsS -o "$(echo "$family" | tr -d ' ')-$weight.ttf" "$url"
  done

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl -fsSL -o "$tmp/InterVar.ttf" "https://github.com/google/fonts/raw/main/ofl/inter/Inter%5Bopsz,wght%5D.ttf"
uv run --quiet --with fonttools python - "$tmp/InterVar.ttf" <<'EOF'
import sys
from fontTools.ttLib import TTFont
from fontTools.varLib import instancer

font = instancer.instantiateVariableFont(TTFont(sys.argv[1]), {"opsz": 14, "wght": 450})
font["OS/2"].usWeightClass = 450
names = font["name"]
for name_id in (1, 2, 3, 4, 6, 16, 17, 25):
    names.removeNames(nameID=name_id)
for platform, encoding, language in ((3, 1, 0x409), (1, 0, 0)):
    for name_id, value in {
        1: "Inter Book", 2: "Regular", 3: "4.001;RSMS;Inter-Book", 4: "Inter Book",
        6: "Inter-Book", 16: "Inter", 17: "Book",
    }.items():
        names.setName(value, name_id, platform, encoding, language)
if "STAT" in font:
    del font["STAT"]
font.save("Inter-450.ttf")
EOF

# CoreText groups faces by typographic family (name ID 16, else ID 1). Some Google statics
# name their family "IBM Plex Mono Medium", which would split one CSS family into several,
# so pin ID 16/17 to the CSS family and weight name. Browsers ignore these names: the
# @font-face rule names the family.
uv run --quiet --with fonttools python - <<'EOF'
import glob, re
from fontTools.ttLib import TTFont

styles = {400: "Regular", 450: "Book", 500: "Medium", 600: "SemiBold", 700: "Bold"}
families = {"Inter": "Inter", "SpaceGrotesk": "Space Grotesk", "JetBrainsMono": "JetBrains Mono",
            "IBMPlexMono": "IBM Plex Mono", "SourceCodePro": "Source Code Pro"}
for path in sorted(glob.glob("*-[0-9][0-9][0-9].ttf")):
    stem, weight = re.match(r"(.+)-(\d+)\.ttf", path).groups()
    font = TTFont(path)
    names = font["name"]
    for platform, encoding, language in ((3, 1, 0x409), (1, 0, 0)):
        names.setName(families[stem], 16, platform, encoding, language)
        names.setName(styles[int(weight)], 17, platform, encoding, language)
    font.save(path)
EOF

for dir in inter spacegrotesk jetbrainsmono ibmplexmono sourcecodepro; do
  curl -fsSL -o "OFL-$dir.txt" "https://github.com/google/fonts/raw/main/ofl/$dir/OFL.txt"
done
