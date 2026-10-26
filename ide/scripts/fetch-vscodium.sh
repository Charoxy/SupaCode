#!/usr/bin/env bash
# Récupère (ou met à jour) les sources de VSCodium dans ide/vscodium,
# puis applique les patchs SupaCode de ide/patches/*.patch.
#
# Variables :
#   VSCODIUM_REF  branche ou tag à récupérer (défaut : master)
set -euo pipefail

IDE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="$IDE_DIR/vscodium"
PATCHES="$IDE_DIR/patches"
REPO="https://github.com/VSCodium/vscodium.git"
REF="${VSCODIUM_REF:-master}"

if [ -d "$TARGET/.git" ]; then
  echo "Mise à jour de VSCodium ($REF)…"
  git -C "$TARGET" fetch --depth 1 origin "$REF"
  git -C "$TARGET" checkout --force FETCH_HEAD
  git -C "$TARGET" clean -fd
else
  echo "Clonage de VSCodium ($REF)…"
  git clone --depth 1 --branch "$REF" "$REPO" "$TARGET"
fi

shopt -s nullglob
patches=("$PATCHES"/*.patch)
if [ ${#patches[@]} -eq 0 ]; then
  echo "Aucun patch à appliquer."
  exit 0
fi

for patch in "${patches[@]}"; do
  echo "Application de $(basename "$patch")…"
  git -C "$TARGET" apply --whitespace=nowarn "$patch"
done
echo "VSCodium prêt dans $TARGET."
