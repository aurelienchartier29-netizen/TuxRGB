# Flatpak

`io.github.tuxrgb.TuxRGB.yml` (dans ce dossier) construit, hors ligne :

1. hidapi 0.14.0 et Mbed TLS 3.6.2 (absents du runtime KDE) ;
2. OpenRGB 1.0 depuis `third_party/openrgb`, installé dans
   `/app/lib/tuxrgb/openrgb/openrgb` (là où TuxRGB le cherche) ;
3. TuxRGB et `tuxrgb-helper` avec les crates de `cargo-sources.json`.

```sh
flatpak install flathub org.kde.Sdk//6.10 org.kde.Platform//6.10 \
  org.freedesktop.Sdk.Extension.rust-stable//25.08 org.freedesktop.Sdk.Extension.llvm20//25.08
flatpak-builder --user --install --force-clean build-flatpak packaging/flatpak/io.github.tuxrgb.TuxRGB.yml
flatpak run io.github.tuxrgb.TuxRGB
```

Après toute modification de `Cargo.lock` :

```sh
curl -LO https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py
pip install aiohttp tomlkit
python3 flatpak-cargo-generator.py Cargo.lock -o packaging/flatpak/cargo-sources.json
```

Vérifiez à chaque mise à jour que la branche des extensions SDK correspond
à la base freedesktop du runtime KDE choisi (KDE 6.10 → 25.08) et passez à un
runtime plus récent s'il en existe un.

## Avant de soumettre à Flathub

* **Identifiant** : `io.github.tuxrgb.TuxRGB` suppose l'organisation GitHub
  `tuxrgb` et le dépôt `TuxRGB`, publics et accessibles. Si vous publiez
  sous votre compte, renommez l'identifiant partout
  (`io.github.<compte>.TuxRGB` : manifest, `.desktop`, metainfo, icônes,
  action Polkit, `APP_ID` dans `crates/tuxrgb/src/paths.rs`).
* **Manifeste écrit par un humain** : depuis septembre 2026, Flathub refuse
  les manifestes générés ou assistés par IA, même déclarés, et interdit aux
  agents IA d'ouvrir ou d'automatiser les PR de soumission. Ce fichier est une
  référence pour les builds locaux : réécrivez-le vous-même avant soumission
  et soumettez la PR vous-même.
* **`--talk-name=org.freedesktop.Flatpak`** permet à l'assistant de lancer
  `flatpak-spawn --host pkexec tuxrgb-helper` pour installer les règles udev
  sur l'hôte. C'est une sortie du bac à sable qui sera discutée en revue. Sans
  cette permission, l'assistant affiche les commandes à exécuter à la main
  (comme le Flatpak officiel d'OpenRGB), le reste de l'application fonctionne.
* **`--device=all`** est indispensable (`/dev/hidraw*`, `/dev/i2c-*`, USB).
* **`--share=network`** sert uniquement à la connexion locale
  `127.0.0.1:6743` vers le serveur OpenRGB privé.
* Captures d'écran : le metainfo pointe vers `docs/screenshots/` sur la
  branche `main` ; elles doivent être en ligne avant la soumission.
