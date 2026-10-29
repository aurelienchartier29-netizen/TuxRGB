# Développement

[← Retour au README](../README.md)

## Architecture

```
┌──────────────────────── processus tuxrgb (Rust) ────────────────────────┐
│ GUI iced 0.14 (Wayland)  ◄── abonnement « services » (tokio) ──┐        │
│  pages · canevas · thème M3  │  ├─ portail Settings / kdeglobals (accent)│
│  ressorts à 60 i/s seulement │  ├─ tray ksni (StatusNotifierItem)       │
│  pendant les animations      │  ├─ captures PipeWire (si effet réactif)  │
│                              │  ├─ afficheurs (thread, USB HID)          │
│                              │  └─ flux d'aperçu ≤ 30 i/s (fenêtre ouverte)
│ moteur d'effets (thread) ── tick fixe si animé, endormi si statique ───┐ │
│   └─ Arc<Frame> ─► watch ─► pont OpenRGB (thread + runtime dédié) ───┐ │ │
└─────────────────────────────────────────────────────────────────────┼─┼─┘
                       SDK OpenRGB v6, 127.0.0.1:6743 (privé)         │
┌──────────────── openrgb --server (OpenRGB 1.1, embarqué) ───────────▼──┐
│ détection SMBus/I2C · HID · USB · série · réseau → vos périphériques    │
└─────────────────────────────────────────────────────────────────────────┘
```

* **Aperçu immersif** : fenêtre ouverte, le moteur évalue aussi l'effet sur
  une grille de 48 × 27 points couvrant tout le canevas (son propre état,
  même luminosité, saturation et fondus que les LED). L'interface en fait
  une image floutée (agrandissement ×4, trois flous de boîte ≈ gaussien,
  bords adoucis) placée sous les appareils. Rien n'est calculé dans la zone
  de notification.
* **Réactivité des curseurs** : la saturation est appliquée telle quelle et
  la luminosité lissée dans le moteur (constante de temps 35 ms : la première
  image bouge déjà, ~95 % en 0,1 s) au lieu de relancer un fondu à chaque
  pas ; un curseur qui bouge fait rendre une image tout de suite, sans
  attendre le prochain tic (au plus deux fois la cadence) ; les commandes en
  rafale sont appliquées d'un coup avant chaque image ; côté OpenRGB, la dernière
  image gagne toujours (envoi groupé), et la couleur d'un appareil SMBus
  retenue par la limite de 10 écritures/s est envoyée dès qu'elle est due ;
  les mises à jour D-Bus de la zone de notification sont regroupées (au plus
  une toutes les 150 ms, la dernière valeur toujours envoyée).
* **Moteur d'effets** (`tuxrgb-core`) : rendu en espace « canevas » 16:9,
  cadence fixe (30–120 i/s) avec échéance absolue, **0 réveil pour un effet
  statique** (renvoi de maintien toutes les 10 s, et le moteur se rendort dès
  la fin d'un fondu). Saturation (0–200 %, Okhsv : teinte et valeur
  perceptuelles conservées, courbe douce sans écrêtage au-delà de 100 %,
  mémorisée par couleur) puis luminosité appliquées en logiciel sur chaque
  LED ; fondus en virgule flottante (sinus entrée/sortie)
  depuis ce que les LED affichent, y compris au milieu d'un autre fondu.
  Mesuré en debug : < 0,1 ms par image pour ~300 LED.
* **Plugins OpenRGB, en natif** : les fonctions des plugins *Effects*,
  *Visual Map* et *Hardware Sync* sont réimplémentées en Rust dans TuxRGB
  (leurs sources ne sont pas embarquées) : effets avancés, shaders GLSL
  (convention Shadertoy, `~/.config/tuxrgb/shaders/custom.frag`, rendus par
  wgpu en 128×72 puis échantillonnés à la position de chaque LED), carte
  visuelle (rotation, miroir, poignée de redimensionnement, zone d'écran par
  LED) et moniteur matériel (hwmon `k10temp`/`coretemp`/`amdgpu`,
  `/proc/stat`, `/proc/meminfo`, `nvidia-smi`).
* **Pont OpenRGB** (`tuxrgb-openrgb`) : trouve le binaire embarqué, le lance
  en `--server --noautoconnect` avec sa propre configuration
  (`~/.config/tuxrgb/openrgb`, n'interfère pas avec un OpenRGB installé),
  `PR_SET_PDEATHSIG` pour qu'il ne survive jamais à TuxRGB, met chaque
  contrôleur en mode *Direct*, n'envoie que les appareils dont les couleurs
  ont changé, se reconnecte et redémarre le serveur si besoin. Un serveur
  OpenRGB externe (port 6742) peut être choisi dans les réglages. Les
  appareils laissés à leur **effet matériel** reçoivent leur mode une fois,
  puis plus aucune couleur (sauf le noir à l'extinction). Les **lumières
  réseau** (WLED, Philips Hue et WiZ, Govee, LIFX, Nanoleaf, Yeelight,
  Elgato, TP-Link Kasa) sont écrites dans les réglages du serveur privé
  (`DDPDevices`, `PhilipsHueDevices`, `GoveeDevices`… de son
  `OpenRGB.json`, le reste du fichier conservé) avant son lancement, avec
  les détecteurs SMBus de la RAM et des cartes mères (`Detectors`, coupés
  tant que l'utilisateur ne les active pas ; liste vérifiée contre les
  sources d'OpenRGB par un test). La recherche (`network::discover`) parle
  le protocole de chaque marque : mDNS en « legacy unicast » (WLED, Elgato,
  Hue, Nanoleaf), scan multicast Govee, `GetService` LIFX, recherche SSDP
  Yeelight, diffusion WiZ et Kasa. Les contrôleurs lents ont un intervalle
  minimal entre deux mises à jour (`network::min_interval`).
* **Ventilateurs par connecteur** (`tuxrgb-core::lighting`) : un connecteur
  divisé en ventilateurs (parts égales, tailles propres à chacun, ou segments
  d'OpenRGB) ; chaque ventilateur est un élément du canevas (anneau de LED
  dans son propre rectangle, rotation et miroir à lui) qui reste dans son
  appareil, à l'endroit des LED du connecteur, tant qu'il n'a pas été
  déplacé.
* **Effets par composant** (`tuxrgb-core::engine`) : les LED d'un même
  appareil qui jouent le même effet propre forment un groupe, rendu à part
  avec son propre état et sa propre horloge, sur leurs positions étirées sur
  leur propre boîte (une vague traverse le ventilateur ou le clavier
  concerné) ; l'effet global continue sans être réinitialisé quand on règle
  un composant. Les entrées (son, écran, capteurs) suivent les besoins de
  l'effet global et de tous les effets propres.
* **Rotation libre** (`tuxrgb-core::layout`) : chaque élément garde son
  rectangle d'origine et un angle quelconque ; LED, zones, ventilateurs
  ancrés, détection au pointeur et poignées sont calculés dans le repère de
  l'élément, en proportions physiques (un carré reste un carré à 45°). Les
  dispositions enregistrées par la 3.0 (quarts de tour) sont converties.
* **Afficheurs** (`crates/tuxrgb/src/displays`) : un thread trouve les
  écrans (`/dev/hidraw*` via sysfs sous Linux, API HID sous Windows, sans
  hidapi ni libusb), lit les mêmes capteurs que le moteur, dessine les écrans
  couleur en logiciel (tiny-skia + Roboto Flex) et envoie à chacun ce qu'il
  affiche à sa cadence. Protocoles réimplémentés d'après liquidctl (Kraken :
  commandes HID et images en bulk USB via usbfs), deepcool-digital-linux
  (rapports HID de 64 octets) et OpenLinkHub (Corsair : flux JPEG en
  rapports de 1024 octets) ; la séquence du Kraken Z est vérifiée octet par
  octet contre celle des tests de liquidctl. Les règles udev installées par
  l'assistant donnent l'accès à ces appareils à l'utilisateur connecté.
* **Fenêtre fermée = zéro GUI** : fermer la fenêtre détruit la surface
  wgpu/Wayland ; le processus reste dans la zone de notification avec le
  moteur et le pont uniquement.
* **GPU basse consommation** par défaut (`WGPU_POWER_PREF=low`) pour ne pas
  réveiller la carte graphique dédiée des portables ; rendu logiciel
  tiny-skia disponible (`--software-render`).

### Mesures

Build release, 312 LED (setup de démonstration), effet animé à 60 i/s,
machine virtuelle sans GPU (Vulkan logiciel lavapipe, le cas le plus lourd) :

| Situation | TuxRGB (RSS) | OpenRGB embarqué (RSS) | CPU |
|---|---|---|---|
| Zone de notification, fenêtre fermée | 16 Mo | 30 Mo | 0,8 % d'un cœur (TuxRGB), 0 % (OpenRGB) |
| Fenêtre ouverte | 214 Mo (pilote Vulkan logiciel inclus) | 30 Mo | rendu à la demande, ≤ 30 i/s d'aperçu |

Un effet statique ne réveille pas le moteur (0 % CPU). Avec un vrai GPU,
la mémoire de la fenêtre dépend surtout du pilote Mesa/NVIDIA chargé.

### Arborescence

```
Cargo.toml                    workspace (profil release : LTO, codegen-units=1, strip)
crates/
  tuxrgb/                     application (GUI, services, tray, thème, privilèges)
    src/app.rs                état, messages, logique (architecture Elm)
    src/services.rs           services d'arrière-plan exposés en abonnement iced
    src/fans/                 ventilation : mod.rs (réglages, page), linux.rs (client du service),
                              windows.rs (contrôleur, processus de garde)
    src/screen_watch.rs       écrans allumés / éteints (KWin DPMS, Mutter, ext-idle-notify)
    src/displays/             afficheurs : hardware_display.rs (modèles, réglages), hid.rs, usb.rs,
                              deepcool.rs, kraken.rs, corsair.rs, render.rs (dessin des écrans)
    src/theme/                tokens M3 Expressive, ressorts, couleurs HCT, KDE
    src/ui/                   accueil, effets, appareils, afficheurs, ventilation (fans.rs, fan_curve.rs), réglages, crédits, assistant, canevas, sélecteur de couleur,
                              motion (boutons, curseurs, interrupteurs), transition (fondus, dépliage, défilement fluide),
                              backdrop (fond flouté de l'aperçu)
    src/privileges/           diagnostic + lancement pkexec du programme d'aide (Windows : *_windows.rs)
    src/update.rs             mises à jour depuis les releases GitHub (AppImage, installateur Windows)
    src/win/                  Windows : fenêtre système, tray Win32, mode administrateur…
    build.rs                  Windows : icône, version et manifeste liés à tuxrgb.exe
  tuxrgb-core/                modèle, canevas, effets, moteur multithread (sans dépendance GUI)
  tuxrgb-openrgb/             superviseur du serveur OpenRGB + client SDK v6 + règles udev
  tuxrgb-capture/             capture audio PipeWire + FFT, capture d'écran portail + PipeWire
                              (Windows : WASAPI, GDI, compteurs système, superio.rs : contrôleurs de ventilateurs)
  tuxrgb-helper/              programme d'aide root (std uniquement, install|enable-smbus|enable-fans|reset|status),
                              service de ventilation (fand.rs, fancurve.rs, hwmon.rs)
vendor/openrgb2/              client SDK openrgb2 0.4.1 + correctif de compatibilité OpenRGB 1.0
third_party/openrgb/          sources complètes d'OpenRGB 1.1 (non modifiées)
data/                         .desktop, metainfo AppStream, action Polkit, icônes, règles udev de référence
packaging/                    build-appimage.sh, AppRun, run-tuxrgb.sh, flatpak/ (manifest, cargo-sources.json),
                              windows/ (installateur NSIS, icône .ico, manifeste, installateur PawnIO signé dans vendor/)
dist/                         lanceur Bazzite/Aurora (les binaires sont dans les releases GitHub)
scripts/                      génération des icônes, de la liste des licences, de l'archive,
                              windows/ (compilation Windows depuis Linux ou sous Windows)
```

## Compiler

```sh
# Fedora
sudo dnf install rust cargo clang pipewire-devel qt6-qtbase-devel qt6-linguist hidapi-devel libusb1-devel mbedtls-devel patchelf
# Debian / Ubuntu 24.04+
sudo apt install cargo clang libpipewire-0.3-dev qt6-base-dev qt6-l10n-tools libhidapi-dev libusb-1.0-0-dev libmbedtls-dev patchelf

cargo build --release                         # tuxrgb + tuxrgb-helper
cargo test --workspace                        # plus de 200 tests unitaires et d'intégration
python3 scripts/i18n-check.py                 # chaînes d'interface sans traduction anglaise
TUXRGB_OPENRGB=/chemin/openrgb \
  cargo test -p tuxrgb-openrgb -- --ignored   # bout en bout avec un vrai serveur OpenRGB
```

Pendant le développement, `TUXRGB_OPENRGB=/chemin/vers/openrgb cargo run`
utilise un OpenRGB compilé à part. `tuxrgb --screenshot-tour dossier/`
capture chaque page en PNG (documentation, tests visuels).

### AppImage

```sh
packaging/appimage/build-appimage.sh                     # compile aussi OpenRGB (third_party/openrgb)
OPENRGB_BIN=/chemin/openrgb packaging/appimage/build-appimage.sh   # réutilise un binaire
```

Le script compile TuxRGB, le programme d'aide **lié statiquement** (il tourne
en root sur n'importe quelle distribution, et depuis l'AppImage il est copié
dans `$XDG_RUNTIME_DIR/tuxrgb/` car root ne peut pas lire un montage FUSE),
embarque OpenRGB avec ses bibliothèques Qt 6/hidapi/libusb/mbedtls
(`RUNPATH=$ORIGIN/lib`, aucune variable `LD_LIBRARY_PATH` exportée), et
n'embarque **jamais** Wayland, Vulkan/GL, PipeWire ni glibc : ce sont ceux du
système.

### Flatpak

```sh
flatpak-builder --user --install --force-clean build-flatpak packaging/flatpak/io.github.tuxrgb.TuxRGB.yml
flatpak run io.github.tuxrgb.TuxRGB
```

À lire **avant toute soumission à Flathub** : [`packaging/flatpak/README.md`](../packaging/flatpak/README.md)
(identifiant `io.github.tuxrgb.TuxRGB` et dépôt GitHub correspondant,
politique Flathub interdisant les manifestes écrits ou assistés par IA,
permission `org.freedesktop.Flatpak` nécessaire à l'assistant d'accès matériel).

### Windows

Depuis Linux (compilation croisée) ou sous Windows :

```sh
# Depuis Linux (compilation croisée) : mingw-w64 + nsis
bash scripts/windows/build-windows.sh    # → dist/TuxRGB-<version>-Windows-Setup.exe
```

```powershell
# Sous Windows : Rust (rustup) + NSIS (installé via winget si absent)
scripts\windows\build-windows.cmd       # double-clic, ou build-windows.ps1
```
