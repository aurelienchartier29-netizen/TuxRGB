#!/usr/bin/env bash
# Publie TuxRGB sur GitHub en une seule commande :
#   1. installe l'outil GitHub (gh) s'il manque,
#   2. connecte ce terminal à votre compte GitHub (une fois, via le navigateur),
#   3. configure Git (nom + adresse e-mail privée « noreply » de GitHub),
#   4. crée le dépôt s'il n'existe pas, sa description, ses sujets et son lien,
#   5. met les liens du projet (README, métadonnées) à l'adresse de votre dépôt,
#   6. envoie tout le code source,
#   7. publie la release vX.Y.Z avec l'AppImage et son .sha256,
#   8. publie la pré-version Windows vX.Y.Z-win-beta avec l'installateur
#      .exe, s'il est dans dist/ (bash scripts/windows/build-windows.sh).
#
# Utilisation (depuis le dossier où sont les fichiers téléchargés, ou depuis
# le dossier du projet) :
#   bash publier-github.sh            # dépôt « TuxRGB »
#   bash publier-github.sh MonDepot   # autre nom de dépôt
#
# Relancer le script ne casse rien : il met à jour ce qui existe déjà.
set -euo pipefail

REPO_NAME="${1:-TuxRGB}"
DESCRIPTION="Éclairage RGB pour Linux : Wayland natif, Material 3 Expressive, OpenRGB intégré (AppImage prête à l'emploi)"
TOPICS=(rgb openrgb linux wayland kde-plasma gnome rust iced appimage argb lighting)

bleu() { printf '\n\033[1;34m==> %s\033[0m\n' "$*"; }
ok() { printf '\033[1;32m✔ %s\033[0m\n' "$*"; }
erreur() { printf '\033[1;31m✘ %s\033[0m\n' "$*" >&2; exit 1; }

# ---------------------------------------------------------------------------
# 0. Trouver le projet (et décompresser l'archive si besoin)
# ---------------------------------------------------------------------------
bleu "Recherche du projet TuxRGB"
DL="${XDG_DOWNLOAD_DIR:-$HOME/Téléchargements}"
[ -d "$DL" ] || DL="$HOME/Downloads"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

PROJECT=""
# L'archive la plus récente téléchargée (les navigateurs renomment les
# téléchargements répétés : « …zip(1).part00 », « …zip (1).part00 »).
NEWEST=""
for d in "$PWD" "$HERE" "$DL"; do
  for f in "$d"/TuxRGB_FullProject*part00 "$d"/TuxRGB_FullProject*.zip; do
    [ -f "$f" ] || continue
    if [ -z "$NEWEST" ] || [ "$f" -nt "$NEWEST" ]; then
      NEWEST="$f"
    fi
  done
done

if [ -n "$NEWEST" ]; then
  PARTS_DIR="$(cd "$(dirname "$NEWEST")" && pwd)"
  cd "$PARTS_DIR"
  ZIP="$PARTS_DIR/.TuxRGB_FullProject-publication.zip"
  case "$NEWEST" in
    *part00)
      PREFIX="${NEWEST%part00}"
      for n in 01 02; do
        [ -f "${PREFIX}part$n" ] || erreur "Il manque ${PREFIX##*/}part$n à côté de ${NEWEST##*/} : téléchargez les 3 parties."
      done
      cat "${PREFIX}"part0[0-9] > "$ZIP" ;;
    *) cp "$NEWEST" "$ZIP" ;;
  esac
  unzip -tq "$ZIP" >/dev/null || erreur "Archive incomplète ou abîmée (${NEWEST##*/}) : retéléchargez les 3 parties."
  ok "Archive : ${NEWEST##*/} ($(date -r "$NEWEST" '+%d/%m/%Y %H:%M'))"
  # Même dossier qu'avant : un dépôt Git déjà présent y est conservé.
  TARGET="$PARTS_DIR/TuxRGB-2.0.2"
  unzip -oq "$ZIP" -d "$TARGET"
  rm -f "$ZIP"
  PROJECT="$TARGET/TuxRGB"
else
  for candidate in "$PWD" "$HERE/.." "$HERE"; do
    if [ -f "$candidate/Cargo.toml" ] && [ -d "$candidate/crates/tuxrgb" ]; then
      PROJECT="$(cd "$candidate" && pwd)"
      break
    fi
  done
  [ -n "$PROJECT" ] || erreur "Archive TuxRGB introuvable : placez ce script à côté de TuxRGB_FullProject.zip.part00, .part01, .part02 (ou dans le dossier du projet)."
fi
cd "$PROJECT"
VERSION="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
APPIMAGE="dist/TuxRGB-$VERSION-x86_64.AppImage"
[ -f "$APPIMAGE" ] || erreur "$APPIMAGE introuvable dans $PROJECT."
ok "Projet : $PROJECT (version $VERSION)"

# ---------------------------------------------------------------------------
# 1. Outil GitHub (gh)
# ---------------------------------------------------------------------------
bleu "Outil GitHub (gh)"
export PATH="$HOME/.local/bin:$PATH"
if ! command -v gh >/dev/null 2>&1; then
  if command -v brew >/dev/null 2>&1; then
    brew install gh
  else
    # Sans Homebrew : la version officielle, dans ~/.local/bin (rien à
    # installer dans le système, compatible Bazzite / Silverblue).
    command -v curl >/dev/null 2>&1 || erreur "curl est nécessaire pour télécharger gh."
    url="$(curl -fsSL https://api.github.com/repos/cli/cli/releases/latest |
      grep -o 'https://[^"]*linux_amd64\.tar\.gz' | head -1)"
    [ -n "$url" ] || erreur "Impossible de trouver le téléchargement de gh."
    tmp="$(mktemp -d)"
    curl -fsSL "$url" | tar -xz -C "$tmp"
    mkdir -p "$HOME/.local/bin"
    install -m 755 "$tmp"/gh_*/bin/gh "$HOME/.local/bin/gh"
    rm -rf "$tmp"
  fi
fi
ok "$(gh --version | head -1)"

# ---------------------------------------------------------------------------
# 2. Connexion au compte GitHub
# ---------------------------------------------------------------------------
bleu "Connexion à GitHub"
if ! gh auth status --hostname github.com >/dev/null 2>&1; then
  echo "Un code va s'afficher : copiez-le, puis validez-le dans le navigateur qui s'ouvre."
  gh auth login --hostname github.com --git-protocol https --web --scopes "repo,workflow"
fi
gh auth setup-git --hostname github.com
LOGIN="$(gh api user --jq .login)"
USER_ID="$(gh api user --jq .id)"
ok "Connecté en tant que $LOGIN"

# ---------------------------------------------------------------------------
# 3. Identité Git (adresse privée de GitHub : votre e-mail reste caché)
# ---------------------------------------------------------------------------
bleu "Identité Git"
if [ -z "$(git config --global user.name || true)" ]; then
  NAME="$(gh api user --jq '.name // .login')"
  git config --global user.name "${NAME:-$LOGIN}"
fi
if [ -z "$(git config --global user.email || true)" ]; then
  git config --global user.email "$USER_ID+$LOGIN@users.noreply.github.com"
fi
git config --global init.defaultBranch main
ok "$(git config --global user.name) <$(git config --global user.email)>"

# ---------------------------------------------------------------------------
# 4. Dépôt GitHub
# ---------------------------------------------------------------------------
REPO="$LOGIN/$REPO_NAME"
URL="https://github.com/$REPO"
bleu "Dépôt $URL"
if ! gh repo view "$REPO" >/dev/null 2>&1; then
  gh repo create "$REPO" --public --description "$DESCRIPTION"
  ok "Dépôt créé"
else
  ok "Dépôt existant"
fi
topic_args=()
for t in "${TOPICS[@]}"; do topic_args+=(--add-topic "$t"); done
gh repo edit "$REPO" \
  --description "$DESCRIPTION" \
  --homepage "$URL/releases/latest" \
  --enable-issues \
  "${topic_args[@]}" >/dev/null
ok "Description, sujets et lien de téléchargement configurés"

# ---------------------------------------------------------------------------
# 5. Liens du projet vers votre dépôt
# ---------------------------------------------------------------------------
bleu "Liens du projet"
sed -i "s#https://github.com/tuxrgb/TuxRGB#$URL#g" Cargo.toml data/io.github.tuxrgb.TuxRGB.metainfo.xml
sed -i "s#https://github.com/[A-Za-z0-9_.-]*/TuxRGB#$URL#g" packaging/windows/tuxrgb.nsi packaging/windows/LIRE-MOI-Windows.txt
sed -i "s#https://raw.githubusercontent.com/tuxrgb/TuxRGB/#https://raw.githubusercontent.com/$REPO/#g" data/io.github.tuxrgb.TuxRGB.metainfo.xml
if grep -q "depuis la page \*\*Releases\*\* du dépôt GitHub" README.md; then
  sed -i "s#depuis la page \*\*Releases\*\* du dépôt GitHub#depuis la page [**Releases**]($URL/releases/latest) du dépôt GitHub#" README.md
fi
ok "README, Cargo.toml et métadonnées pointent vers $URL"

# ---------------------------------------------------------------------------
# 6. Code source
# ---------------------------------------------------------------------------
bleu "Envoi du code source"
[ -d .git ] || git init -q
git checkout -q -B main
# Les AppImage et installateurs vont dans les releases, jamais dans le dépôt.
git rm -r -q --cached --ignore-unmatch 'dist/*.AppImage' 'dist/*.AppImage.sha256' \
  'dist/*.exe' 'dist/*.exe.sha256' >/dev/null
git add -A
if git diff --cached --quiet; then
  ok "Rien de nouveau à enregistrer"
else
  git commit -q -m "TuxRGB $VERSION – mise à jour du $(date '+%d/%m/%Y %H:%M')"
  ok "Modifications enregistrées (TuxRGB $VERSION)"
fi
if git remote get-url origin >/dev/null 2>&1; then
  git remote set-url origin "$URL.git"
else
  git remote add origin "$URL.git"
fi
if ! git push -u origin main 2>/tmp/tuxrgb-push.log; then
  cat /tmp/tuxrgb-push.log >&2
  echo
  echo "GitHub contient déjà un historique différent (par exemple un README créé sur le site)."
  read -r -p "Le remplacer par cette version du projet ? [o/N] " answer
  case "$answer" in
    o|O|oui|Oui|y|Y) git push -u origin main --force ;;
    *) erreur "Envoi annulé : rien n'a été modifié sur GitHub." ;;
  esac
fi
ok "Code en ligne : $URL"

# ---------------------------------------------------------------------------
# 7. Release avec l'AppImage
# ---------------------------------------------------------------------------
TAG="v$VERSION"
bleu "Release $TAG"
NOTES="$(mktemp)"
{
  echo "## TuxRGB $VERSION"
  echo
  echo "Éclairage RGB pour Linux : Wayland natif, Material 3 Expressive, moteur OpenRGB intégré (rien d'autre à installer)."
  echo
  # Notes propres à la version (docs/notes-de-version/X.Y.Z.md), si présentes.
  if [ -f "docs/notes-de-version/$VERSION.md" ]; then
    cat "docs/notes-de-version/$VERSION.md"
    echo
  fi
  cat <<EOF
### Installation
1. Téléchargez \`TuxRGB-$VERSION-x86_64.AppImage\` ci-dessous
2. Dans un terminal :
   \`\`\`bash
   chmod +x TuxRGB-$VERSION-x86_64.AppImage
   ./TuxRGB-$VERSION-x86_64.AppImage
   \`\`\`
   Ou intégrez-la au menu avec Gear Lever (préinstallé sur Bazzite).

Nécessite une session Wayland et glibc ≥ 2.39 (Fedora 40+, Bazzite, Aurora, Ubuntu 24.04+, Debian 13, Arch, openSUSE Tumbleweed…).

Vérification : \`sha256sum -c TuxRGB-$VERSION-x86_64.AppImage.sha256\`
EOF
} > "$NOTES"
FILES=("$APPIMAGE")
[ -f "$APPIMAGE.sha256" ] && FILES+=("$APPIMAGE.sha256")
if gh release view "$TAG" --repo "$REPO" >/dev/null 2>&1; then
  # Même version republiée (correctifs) : l'étiquette suit le code qui a
  # produit les nouveaux fichiers, qui remplacent les anciens.
  git tag -f "$TAG" HEAD >/dev/null
  git push -q -f origin "refs/tags/$TAG"
  gh release upload "$TAG" "${FILES[@]}" --repo "$REPO" --clobber
  gh release edit "$TAG" --repo "$REPO" --title "TuxRGB $VERSION" --notes-file "$NOTES" --draft=false --latest
  ok "Release $TAG mise à jour (AppImage remplacée)"
else
  gh release create "$TAG" "${FILES[@]}" --repo "$REPO" --target main \
    --title "TuxRGB $VERSION" --notes-file "$NOTES" --latest
  ok "Release $TAG publiée"
fi
rm -f "$NOTES"

# ---------------------------------------------------------------------------
# 8. Pré-version Windows (Windows Beta Build)
# ---------------------------------------------------------------------------
WIN_TAG="v$VERSION-win-beta"
WIN_SETUP="dist/TuxRGB-$VERSION-win-beta-Setup.exe"
if [ -f "$WIN_SETUP" ]; then
  bleu "Pré-version Windows $WIN_TAG"
  WIN_NOTES="$(mktemp)"
  {
    echo "## TuxRGB $VERSION-win-beta — Windows Beta Build"
    echo
    if [ -f "docs/notes-de-version/$VERSION-win-beta.md" ]; then
      cat "docs/notes-de-version/$VERSION-win-beta.md"
    else
      echo "Version bêta pour Windows 10 / 11. Linux reste la version de référence."
    fi
  } > "$WIN_NOTES"
  WIN_FILES=("$WIN_SETUP")
  [ -f "$WIN_SETUP.sha256" ] && WIN_FILES+=("$WIN_SETUP.sha256")
  if gh release view "$WIN_TAG" --repo "$REPO" >/dev/null 2>&1; then
    git tag -f "$WIN_TAG" HEAD >/dev/null
    git push -q -f origin "refs/tags/$WIN_TAG"
    gh release upload "$WIN_TAG" "${WIN_FILES[@]}" --repo "$REPO" --clobber
    gh release edit "$WIN_TAG" --repo "$REPO" --title "TuxRGB $VERSION-win-beta (Windows Beta Build)" \
      --notes-file "$WIN_NOTES" --draft=false --prerelease --latest=false
    ok "Pré-version $WIN_TAG mise à jour"
  else
    gh release create "$WIN_TAG" "${WIN_FILES[@]}" --repo "$REPO" --target main \
      --title "TuxRGB $VERSION-win-beta (Windows Beta Build)" --notes-file "$WIN_NOTES" \
      --prerelease --latest=false
    ok "Pré-version $WIN_TAG publiée (la release Linux reste « Latest »)"
  fi
  rm -f "$WIN_NOTES"
fi

# Brouillons de release abandonnés (créés à la main sur le site).
DRAFTS="$(gh release list --repo "$REPO" --json tagName,isDraft --jq '.[] | select(.isDraft) | .tagName' || true)"
if [ -n "$DRAFTS" ]; then
  echo "Brouillons de release trouvés : $(echo "$DRAFTS" | tr '\n' ' ')"
  read -r -p "Les supprimer ? [o/N] " answer
  case "$answer" in
    o|O|oui|Oui|y|Y)
      while read -r d; do [ -n "$d" ] && gh release delete "$d" --repo "$REPO" --yes; done <<<"$DRAFTS"
      ok "Brouillons supprimés" ;;
  esac
fi

bleu "Terminé"
echo "  Dépôt   : $URL"
echo "  AppImage : $URL/releases/latest"
if [ -f "$WIN_SETUP" ]; then
  echo "  Windows  : $URL/releases/tag/$WIN_TAG"
fi
