# TuxRGB pour Windows 10 / 11

[← Retour au README](../README.md)

La version Windows (10 / 11, 64 bits) est publiée **en même temps que Linux,
dans la même release**, avec les mêmes correctifs.

**Installation** : télécharger `TuxRGB-…-Windows-Setup.exe` (page
[Releases](https://github.com/aurelienchartier29-netizen/TuxRGB/releases/latest)), double-cliquer, suivre l'installateur. Il installe TuxRGB dans
`C:\Program Files\TuxRGB`, crée les raccourcis, puis :

* fait télécharger par TuxRGB lui-même (HTTPS, sans aucun script) **OpenRGB
  1.1** pour Windows depuis sa source officielle (Codeberg / openrgb.org), le
  même moteur que celui embarqué dans l'AppImage, lancé par TuxRGB en serveur
  SDK privé ;
* installe le pilote **PawnIO** (accès SMBus : carte mère, RAM — facultatif),
  dont l'installateur officiel, signé par son auteur, est inclus ;
* et configure le **mode administrateur** (facultatif) : une tâche planifiée
  relance TuxRGB avec les droits administrateur, sans invite UAC à chaque
  démarrage, condition imposée par Windows pour lire le SMBus.

**Mises à jour** : Réglages → Mises à jour → Rechercher. TuxRGB télécharge
le nouvel installateur, vérifie sa somme SHA-256 et le lance en mode mise à
jour (`/S /UPDATE`, une confirmation Windows) : installation silencieuse qui
ne remplace que TuxRGB (OpenRGB, PawnIO, mode administrateur et raccourcis
gardés tels quels), puis TuxRGB redémarre tout seul.

| Réglages : mises à jour | Installateur |
|---|---|
| ![Réglages Windows](screenshots/windows/01-reglages-mises-a-jour.png) | ![Installateur](screenshots/windows/04-installateur.png) |
| ![À propos](screenshots/windows/02-a-propos.png) | ![Zone de notification](screenshots/windows/03-zone-de-notification.png) |

Ce qui est **identique** à Linux : toute l'interface iced / Material 3
Expressive, les animations, les effets, les profils, le canevas spatial
(ventilateurs par connecteur, rotation, masquage), les effets matériels, les
lumières réseau (WLED, Hue, WiZ, Govee, LIFX, Nanoleaf, Yeelight, Elgato, Kasa), la prudence sur la RAM et le moteur OpenRGB. Ce qui **change**, par compilation conditionnelle
(`#[cfg(target_os = "windows")]`, dossier `crates/tuxrgb/src/win/`) :

| Linux | Windows |
|---|---|
| Visualiseur audio : moniteur PipeWire | Capture **WASAPI** en boucle de la sortie par défaut (suit le changement de casque / enceintes) |
| Synchro écran : portail ScreenCast + PipeWire | Écran principal par GDI (les jeux en plein écran exclusif restent noirs : utiliser « fenêtré sans bordure ») |
| Zone de notification : StatusNotifierItem (ksni) | Icône **Win32** (`Shell_NotifyIconW`), même menu, notifications en bulles / toasts |
| Extinction / veille : inhibiteur logind (400 ms au plus) | `WM_QUERYENDSESSION` / `WM_ENDSESSION` / `WM_POWERBROADCAST` : toutes les LED à 0 % à l'arrêt, au redémarrage, à la fermeture de session et à la mise en veille, en moins d'une demi-seconde, restaurées au réveil et au démarrage suivant |
| Accent : portail, kdeglobals, gsettings | **WinRT** `UISettings.GetColorValue(Accent)`, repli registre (`DWM\AccentColor`), mode clair / sombre et « effets d'animation » de Windows, suivis en direct (`WM_SETTINGCHANGE`) |
| Instance unique : D-Bus | Mutex nommé + `WM_COPYDATA` |
| Démarrage avec la session : `~/.config/autostart` | Clé `HKCU\…\Run` |
| Accès matériel : udev, i2c-dev, Polkit | PawnIO + mode administrateur (UAC une seule fois) |
| Ventilation : service système `tuxrgb-fans` (hwmon `pwmN`), actif dès le démarrage | Contrôleurs ITE / Nuvoton pilotés par TuxRGB en mode administrateur via PawnIO (`LpcIO`), processus de garde qui rend les ventilateurs à la carte mère si TuxRGB s'arrête brutalement ; ventilateurs du second contrôleur des cartes Gigabyte en lecture seule |
| LED éteintes avec l'écran : DPMS de KWin, Mutter, ou inactivité | `GUID_CONSOLE_DISPLAY_STATE` (état de l'écran donné par Windows) |
| Capteurs : hwmon, `/proc`, RAPL | Charge et fréquence CPU, mémoire ; charge et température de toute carte graphique par Windows (compteurs du Gestionnaire des tâches, noyau graphique), détails NVIDIA via `nvidia-smi` ; température et puissance du CPU (Ryzen, Intel Core) par le pilote signé **PawnIO** en mode administrateur, ou **LibreHardwareMonitor** s'il est ouvert |

Réglages : `%APPDATA%\tuxrgb` — journaux et rapport matériel :
`%LOCALAPPDATA%\tuxrgb`. Les retours sont les bienvenus (onglet *Issues*).
