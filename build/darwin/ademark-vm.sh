#!/usr/bin/env bash
# Benches this branch's Recur in the ADEMark macOS VM: package, install, re-pin, Track A, summary.
# VM setup, calibration and gotchas: .agents/skills/ademark-bench/SKILL.md
#   ADEMARK_VM      ssh target (default ademark@192.168.64.2)
#   ADEMARK_VM_KEY  ssh key (default ~/.ssh/ademark_vm)
#   ADEMARK_ADES    ADEs to run in order (default "recur"), e.g. "super-engineering recur"
#   ADEMARK_FULL=1  full Track A instead of --smoke
#   SKIP_PACKAGE=1  reuse target/package/Recur.dmg
set -euo pipefail

VM=${ADEMARK_VM:-ademark@192.168.64.2}
KEY=${ADEMARK_VM_KEY:-$HOME/.ssh/ademark_vm}
ADES=${ADEMARK_ADES:-recur}
MODE=--smoke
[[ ${ADEMARK_FULL:-} == 1 ]] && MODE=
TAG=bench-$(date +%Y%m%d-%H%M%S)
SSH=(ssh -i "$KEY" -o IdentitiesOnly=yes -o BatchMode=yes -o ConnectTimeout=10 "$VM")
root=$(cd "$(dirname "$0")/../.." && pwd)

[[ ${SKIP_PACKAGE:-} == 1 ]] || (cd "$root" && mise run macos:package)
scp -i "$KEY" -o IdentitiesOnly=yes -o BatchMode=yes -q "$root/target/package/Recur.dmg" "$VM:/tmp/Recur.dmg"

"${SSH[@]}" "ADES='$ADES' MODE='$MODE' TAG='$TAG' bash -s" <<'EOF'
set -euo pipefail
eval "$(/opt/homebrew/bin/brew shellenv)"
if pgrep -f "bun bench/cli.ts run" >/dev/null; then echo "a bench run is already going in the VM" >&2; exit 1; fi

osascript -e 'quit app "Recur"' 2>/dev/null || true
sleep 2
hdiutil attach -nobrowse -quiet -mountpoint /tmp/recurdmg /tmp/Recur.dmg
rm -rf /Applications/Recur.app
ditto /tmp/recurdmg/Recur.app /Applications/Recur.app
hdiutil detach -quiet /tmp/recurdmg

cd ~/ADEMark
info=$(bun -e 'import {recur} from "./bench/adapters/recur.ts"; console.log(JSON.stringify(await recur.installCheck()))')
python3 -I - "$info" <<'PY'
import json, sys
i = json.loads(sys.argv[1]); p = "bench/adapters/profiles/recur.json"; d = json.load(open(p))
d["app"]["version"], d["app"]["bundleSha256"] = i["version"], i["binarySha256"]
open(p, "w").write(json.dumps(d, indent=2) + "\n")
PY
git diff --quiet || git commit -qam "recur: pin $(jq -r .app.version bench/adapters/profiles/recur.json)"
echo "installed $(jq -r .app.version bench/adapters/profiles/recur.json)"

# Real runs must start from Ghostty: it holds the Accessibility and Screen Recording grants.
cat > ~/run-seq.sh <<'SH'
#!/bin/zsh -f
export PATH="/opt/homebrew/bin:$HOME/.local/bin:/usr/bin:/bin:/usr/sbin:/sbin" ADEMARK_AGENT=off
tag=$1 mode=$2; shift 2
cd ~/ADEMark
for a in "$@"; do bun bench/cli.ts run --ade $a --tracks A --agent claude ${=mode} > /tmp/$tag-$a.log 2>&1; done
touch /tmp/$tag.done
SH
chmod +x ~/run-seq.sh
open -na Ghostty --args -e ~/run-seq.sh "$TAG" "$MODE" $ADES
EOF

echo "running $ADES ($TAG) in the VM; leave it and the host quiet"
until "${SSH[@]}" "test -f /tmp/$TAG.done" 2>/dev/null; do sleep 60; done

"${SSH[@]}" "TAG='$TAG' ADES='$ADES' bash -s" <<'EOF'
for a in $ADES; do
  dir=$(sed -n 's/^wrote //p' /tmp/$TAG-$a.log | tail -1)
  echo "== $a  ${dir:-no results, see /tmp/$TAG-$a.log}"
  [[ -n $dir ]] || continue
  python3 -I - "$dir/rows.jsonl" <<'PY'
import json, sys
keep = {"A1": "cold", "A2": "relaunch", "A3": "default-mode", "A4": "total", "A5": "total", "A6": "front",
        "A7": "total-p95", "A8": "p95", "A9": "no-sessions", "A10": "mb-per-thread", "A11": "open"}
for line in open(sys.argv[1]):
    r = json.loads(line)
    if keep.get(r["test_id"]) != r["variant"]:
        continue
    v = r.get("value")
    v = f"{v:.2f} {r['unit']}" if isinstance(v, (int, float)) else "-"
    print(f"{r['test_id']:4} {r['variant']:14} rep {r.get('rep')}  {v:>14}  {'' if r.get('valid') else r.get('invalid_reason')}")
PY
done
EOF
