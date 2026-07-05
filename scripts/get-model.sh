#!/usr/bin/env bash
# Downloads a whisper.cpp GGML model into ../models/. No Python; just curl.
# Usage: ./get-model.sh [medium|large-v3|small|base|tiny]   (default: medium)
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p models

NAME="${1:-medium}"
case "$NAME" in
  tiny|base|small|medium|large-v3) ;;
  *) echo "unknown model '$NAME' (use tiny|base|small|medium|large-v3)" >&2; exit 1 ;;
esac

FILE="ggml-${NAME}.bin"
URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/${FILE}"
DEST="models/${FILE}"

if [[ -f "$DEST" ]]; then
  echo "Already have $DEST"; exit 0
fi

echo ">> Downloading $FILE ..."
echo "   (medium ~1.5GB, large-v3 ~3GB — first run only)"
curl -L --fail --progress-bar -o "${DEST}.part" "$URL"
mv "${DEST}.part" "$DEST"
echo ">> Saved $DEST"
ls -lh "$DEST"
