#!/usr/bin/env bash
# 立ち絵の原画(<state>1.png … <state>4.png)を 1 枚のシート(6 行 × 4 列)に組む。
# アプリには入らない、素材を書き出すための一度きりの道具。仕様は docs/character-sheet.md。
#
#   ./scripts/build-character-sheet.sh                       public/character から組み、設定フォルダへ書く
#   ./scripts/build-character-sheet.sh --out /tmp/sheet.png  出力先を変える
#   ./scripts/build-character-sheet.sh --frame 360x480       1 コマの寸法を変える(既定 480x640)
#
# 全コマを「同じ 1 つの枠」で切るので、原画どうしの位置関係はそのまま保たれる。
# 揃っていない原画は揃わないまま出る(それが分かるよう、組む前に足元のずれを報告する)。
# ImageMagick(magick)が要る。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
STATES=(idle thinking working waiting success error)
COLS=4

IN="$ROOT/public/character"
OUT="$HOME/Library/Application Support/com.nagatadaichi.tauriapp/character.png"
FRAME="480x640"
MARGIN=4
FORCE=0

while [ $# -gt 0 ]; do
  case "$1" in
    --in) IN="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --frame) FRAME="$2"; shift 2 ;;
    --margin) MARGIN="$2"; shift 2 ;;
    --force) FORCE=1; shift ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done

command -v magick >/dev/null || { echo "magick が要ります: brew install imagemagick" >&2; exit 1; }
FW="${FRAME%x*}"; FH="${FRAME#*x}"
[ "$FW" -gt 0 ] && [ "$FH" -gt 0 ] || { echo "--frame は 480x640 の形で指定してください" >&2; exit 2; }
if [ -e "$OUT" ] && [ "$FORCE" -eq 0 ]; then
  echo "既にあります: $OUT" >&2
  echo "上書きするなら --force を付けてください。" >&2
  exit 1
fi

# --- 1. 原画を集め、キャンバスと足元を測る -------------------------------------
# α が 50% 以上の画素の外接矩形を「絵の実寸」とみなす。
PRESENT=""
MISSING=""
UL=999999; UT=999999; UR=0; UB=0   # 全コマの和集合(left/top/right/bottom)
CW=0; CH=0
declare -a BOTTOMS=()

for st in "${STATES[@]}"; do
  have=1
  for i in $(seq 1 $COLS); do [ -f "$IN/$st$i.png" ] || have=0; done
  if [ "$have" -eq 0 ]; then MISSING="$MISSING $st"; continue; fi
  PRESENT="$PRESENT $st"
  sb=0
  for i in $(seq 1 $COLS); do
    f="$IN/$st$i.png"
    read -r cw ch bbox <<<"$(magick "$f" -format "%w %h " info: && magick "$f" -alpha extract -threshold 50% -format "%@" info:)"
    if [ "$CW" -eq 0 ]; then CW=$cw; CH=$ch; fi
    [ "$cw" = "$CW" ] && [ "$ch" = "$CH" ] || { echo "キャンバスが揃っていません: $f は ${cw}x${ch}(他は ${CW}x${CH})" >&2; exit 1; }
    w="${bbox%%x*}"; rest="${bbox#*x}"; h="${rest%%+*}"
    x="$(echo "$bbox" | sed 's/.*+\([0-9]*\)+[0-9]*/\1/')"
    y="${bbox##*+}"
    [ "$x" -lt "$UL" ] && UL=$x || true
    [ "$y" -lt "$UT" ] && UT=$y || true
    [ $((x + w)) -gt "$UR" ] && UR=$((x + w)) || true
    [ $((y + h)) -gt "$UB" ] && UB=$((y + h)) || true
    [ $((y + h)) -gt "$sb" ] && sb=$((y + h)) || true
  done
  BOTTOMS+=("$st:$sb")
done

[ -n "$PRESENT" ] || { echo "$IN に原画がありません" >&2; exit 1; }

# --- 2. 足元のずれを報告する --------------------------------------------------
sorted=$(for b in "${BOTTOMS[@]}"; do echo "${b#*:}"; done | sort -n)
med=$(echo "$sorted" | awk '{a[NR]=$1} END {print a[int((NR+1)/2)]}')
tol=$((CH / 100))
echo "キャンバス ${CW}x${CH} / 足元の基準 y=${med}(許容 ±${tol})"
for b in "${BOTTOMS[@]}"; do
  st="${b%%:*}"; v="${b#*:}"; d=$((v - med)); [ "$d" -lt 0 ] && d=$((-d))
  if [ "$d" -gt "$tol" ]; then
    echo "  ! $st: 足元 y=$v(基準から $((v - med)) px ずれている — 描き直しの候補)"
  else
    echo "    $st: 足元 y=$v"
  fi
done
for st in $MISSING; do echo "    $st: 原画なし → 透明の行にする"; done

# --- 3. 切り出す枠を 1 つ決める(和集合 + 余白 → 3:4) --------------------------
mx=$(( (UR - UL) * MARGIN / 100 )); my=$(( (UB - UT) * MARGIN / 100 ))
L=$((UL - mx)); T=$((UT - my)); R=$((UR + mx)); B=$((UB + my))
BW=$((R - L)); BH=$((B - T))
if [ $((BW * FH)) -lt $((BH * FW)) ]; then
  nw=$(( (BH * FW + FH - 1) / FH )); L=$(( L - (nw - BW) / 2 )); BW=$nw
else
  nh=$(( (BW * FH + FW - 1) / FW )); T=$(( T - (nh - BH) / 2 )); BH=$nh
fi
echo "切り出す枠 ${BW}x${BH}+${L}+${T}(全コマ共通)→ 1 コマ ${FW}x${FH}"

# --- 4. 組む ------------------------------------------------------------------
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
rows=()
for st in "${STATES[@]}"; do
  row="$TMP/row-$st.png"
  if [ -f "$IN/${st}1.png" ]; then
    frames=()
    for i in $(seq 1 $COLS); do
      out="$TMP/$st$i.png"
      # 透明の台紙に原画を置いてから切るので、枠がキャンバスの外へ出ても破綻しない
      magick -size "${BW}x${BH}" xc:none "$IN/$st$i.png" \
        -geometry "$(printf '%+d%+d' $((-L)) $((-T)))" -composite \
        -resize "${FW}x${FH}!" "$out"
      frames+=("$out")
    done
    magick "${frames[@]}" -background none +append "$row"
  else
    magick -size "$((FW * COLS))x${FH}" xc:none "$row"
  fi
  rows+=("$row")
done

mkdir -p "$(dirname "$OUT")"
magick "${rows[@]}" -background none -append +repage -depth 8 -strip "$OUT"

# --- 5. 仕様どおりか確かめる --------------------------------------------------
read -r w h <<<"$(magick "$OUT" -format "%w %h" info:)"
[ $((w % COLS)) -eq 0 ] && [ $((h % ${#STATES[@]})) -eq 0 ] \
  || { echo "組み上がりが仕様を満たしていません: ${w}x${h}" >&2; exit 1; }
echo "書きました: $OUT(${w}x${h}、$(du -h "$OUT" | cut -f1))"
