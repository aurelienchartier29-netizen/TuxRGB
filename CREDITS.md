# Crédits & licences open source

TuxRGB existe grâce à ces projets libres. La même liste est affichée dans
l'application : **Réglages → À propos → Crédits & licences open source**.

## Embarqués dans les paquets TuxRGB

| Projet | Rôle | Auteurs | Licence |
|---|---|---|---|
| [OpenRGB 1.1](https://gitlab.com/CalcProgrammer1/OpenRGB) | Moteur matériel : détection et pilotage de tous les périphériques (SMBus/I2C, HID, USB, série, réseau), lancé en serveur SDK privé | Adam Honse (CalcProgrammer1) et les contributeurs OpenRGB | GPL-2.0-or-later |
| [Tux](https://isc.tamu.edu/~lewing/linux/) | Manchot de l'icône de l'application (sur fond RGB) | Larry Ewing (lewing@isc.tamu.edu), créé avec The GIMP | Utilisation et modification libres avec mention de l'auteur |
| [Qt 6](https://www.qt.io/) | Bibliothèques utilisées par OpenRGB (mode serveur sans interface) | The Qt Company et contributeurs | LGPL-3.0 |
| [hidapi](https://github.com/libusb/hidapi) | Accès HID d'OpenRGB | Alan Ott, Signal 11 Software, libusb/hidapi | BSD-3-Clause / GPL-3.0 / HIDAPI (au choix) |
| [libusb](https://libusb.info/) | Accès USB d'OpenRGB | contributeurs libusb | LGPL-2.1-or-later |
| [Mbed TLS](https://github.com/Mbed-TLS/mbedtls) | Cryptographie des contrôleurs réseau d'OpenRGB | Trusted Firmware / Arm | Apache-2.0 |
| [openrgb2](https://github.com/Achtuur/openrgb-rs2) | Client Rust du SDK OpenRGB (v6) – copie corrigée dans `vendor/openrgb2` | Achtuur et contributeurs, d'après openrgb-rs (nicoulaj) | GPL-2.0 |
| [iced](https://iced.rs/) | Interface graphique, rendu wgpu / tiny-skia, Wayland natif | Héctor Ramón et contributeurs | MIT |
| [wgpu](https://wgpu.rs/) | Rendu GPU | gfx-rs | MIT OR Apache-2.0 |
| [winit](https://github.com/rust-windowing/winit), [smithay-client-toolkit](https://github.com/Smithay/client-toolkit) | Fenêtrage Wayland | rust-windowing, Smithay | Apache-2.0, MIT |
| [ksni](https://github.com/iovxw/ksni) | Zone de notification (StatusNotifierItem) | iovxw | Unlicense |
| [ashpd](https://github.com/bilelmoussaoui/ashpd), [zbus](https://github.com/dbus2/zbus) | Portails XDG, D-Bus | Bilal Elmoussaoui, Zeeshan Ali Khan et contributeurs | MIT |
| [pipewire-rs](https://gitlab.freedesktop.org/pipewire/pipewire-rs) | Liaison PipeWire (audio, écran) | contributeurs pipewire-rs | MIT |
| [material-colors](https://github.com/Aiving/material-colors) | Couleurs dynamiques Material 3 (HCT) | Aiving, d'après material-color-utilities (Google) | MIT OR Apache-2.0 |
| [Material Symbols Rounded](https://github.com/google/material-design-icons) | Icônes (sous-ensemble) | Google | Apache-2.0 |
| [realfft](https://github.com/HEnquist/realfft), [rustfft](https://github.com/ejmahler/RustFFT) | FFT du visualiseur audio | Henrik Enquist, Elliott Mahler et contributeurs | MIT OR Apache-2.0 |
| [naga](https://github.com/gfx-rs/wgpu/tree/trunk/naga) | Compilation des shaders GLSL de l'effet « Shader GLSL » | gfx-rs | MIT OR Apache-2.0 |
| [tokio](https://tokio.rs/) | Exécution asynchrone | Tokio contributors | MIT |

La liste exhaustive des ≈300 bibliothèques Rust, avec version et licence, est
générée depuis `Cargo.lock` par `scripts/gen-third-party.py`
(`crates/tuxrgb/assets/third_party.tsv`) et consultable dans l'application.

## Utilisés par TuxRGB sur votre système (non embarqués)

PipeWire (MIT), xdg-desktop-portal et les portails KDE / GNOME / COSMIC
(LGPL, GPL), Polkit (LGPL), udev/systemd et logind (LGPL), Mesa ou votre pilote
GPU, Wayland (MIT), le serveur de notifications de votre bureau.

## Windows Beta Build

* [windows-rs](https://github.com/microsoft/windows-rs) (Microsoft, MIT OR
  Apache-2.0) : WASAPI, zone de notification, événements d'alimentation,
  registre, WinRT `UISettings` — lié à `tuxrgb.exe`.
* [NSIS](https://nsis.sourceforge.io/) (zlib/libpng) : l'installateur
  `TuxRGB-…-win-beta-Setup.exe`.
* Téléchargés par l'installateur depuis leurs sources officielles, jamais
  redistribués par TuxRGB : OpenRGB 1.1 pour Windows (GPL-2.0-or-later, avec
  Qt 6 LGPL-3.0) et le pilote [PawnIO](https://pawnio.eu) (namazso).
* Utilisés sur le système : WASAPI, GDI, le Planificateur de tâches et
  l'Explorateur Windows.

## Références de conception

* [Material Design 3 Expressive](https://m3.material.io/) (Google) : échelle de
  formes, schémas de mouvement à ressorts, rôles de couleur, typographie.
* Interface inspirée de l'ergonomie des logiciels RGB « canevas » ; TuxRGB
  n'est affilié ni à WhirlwindFX (SignalRGB) ni au projet OpenRGB.
* Plugins de la communauté OpenRGB *Effects*, *Visual Map* et *Hardware
  Sync* : leurs fonctionnalités (effets avancés et shaders, carte visuelle,
  couleurs selon les capteurs) ont servi de référence et sont réimplémentées
  nativement en Rust dans TuxRGB ; aucun code de ces plugins n'est repris ni
  embarqué.
* Couleurs d'accent nommées de GNOME (libadwaita, LGPL) reproduites pour la
  synchronisation sous GNOME ≤ 46.

## Textes de licence

`LICENSE` (GPL-2.0), `licenses/GPL-3.0.txt`, `licenses/LGPL-2.1.txt`,
`licenses/LGPL-3.0.txt`, `licenses/Apache-2.0.txt`, `licenses/MIT.txt`.
