# Installation et premier lancement

[← Retour au README](../README.md)

## Linux

1. Téléchargez `TuxRGB-…-x86_64.AppImage` sur la page
   [Releases](https://github.com/aurelienchartier29-netizen/TuxRGB/releases/latest).
2. Rendez-le exécutable et lancez-le :

   ```sh
   chmod +x TuxRGB-*-x86_64.AppImage
   ./TuxRGB-*-x86_64.AppImage
   ```

   Ou double-cliquez dessus dans le gestionnaire de fichiers. Sur Bazzite,
   **Gear Lever** (préinstallé) l'ajoute au menu des applications. Rien à
   compiler, rien à « layer » avec rpm-ostree sur les systèmes immuables
   (Bazzite, Aurora, Bluefin, Fedora Atomic).

L'AppImage nécessite glibc ≥ 2.39 (Fedora 40+, Ubuntu 24.04+, Debian 13,
Arch, openSUSE Tumbleweed…) et une session Wayland.

Le script [`dist/run-tuxrgb.sh`](../dist/run-tuxrgb.sh), posé à côté de
l'AppImage, peut aussi la lancer, l'installer dans `~/Applications` avec son
entrée de menu (`--install`) ou faire un diagnostic rapide du système
(`--check`).

Ensuite, TuxRGB se met à jour tout seul : **Réglages → Mises à jour →
Rechercher** télécharge la nouvelle AppImage depuis les releases GitHub,
vérifie sa somme SHA-256, la met à la place de l'ancienne (même dossier ; un
nom qui contient le numéro de version est renommé) et redémarre TuxRGB.

## Windows

Voir [WINDOWS.md](WINDOWS.md).

## Premier lancement

1. **Bienvenue**, puis **Diagnostic** : règles udev, `i2c-dev`, pilote SMBus
   du chipset, accès aux `/dev/i2c-*` et `/dev/hidraw*`, moteur OpenRGB.
2. **Accès matériel** : un clic, votre mot de passe administrateur dans la
   fenêtre Polkit de KDE/GNOME. Le programme d'aide `tuxrgb-helper` :
   * installe les règles udev **générées par l'OpenRGB embarqué**
     (`openrgb --print-udev-rules`, au moment de la compilation) et validées
     ligne par ligne (seuls `TAG+="uaccess"`, les étiquettes et les
     `chmod a+w /sys/…` d'OpenRGB sont acceptés : aucun
     `RUN`/`PROGRAM`/`IMPORT` arbitraire). Sans règles fournies par la
     distribution : `/etc/udev/rules.d/60-openrgb.rules`. Si la distribution
     en fournit déjà (`/usr/lib/udev/rules.d/60-openrgb.rules`, ex. Bazzite) :
     seulement les appareils qui y manquent, dans
     `/etc/udev/rules.d/61-tuxrgb-openrgb-extra.rules` – plus de doublon ni
     d'avertissement « Multiple OpenRGB udev rules » ;
   * **aucun droit universel** : l'accès aux `/dev/hidraw*` et `/dev/i2c-*`
     passe uniquement par `TAG+="uaccess"` (ACL de la session active). Les
     quelques règles d'OpenRGB qui rendaient des fichiers de claviers de
     portables ASUS/faustus modifiables par tous (`chmod a+w`) sont retirées,
     et si la distribution les fournit, TuxRGB les annule (`chmod go-w`) ;
     tout `MODE` donnant un droit aux « autres » est refusé ;
   * écrit `/etc/modules-load.d/tuxrgb-i2c.conf` (`i2c-dev` + `i2c-piix4`
     ou `i2c-i801` selon le contrôleur SMBus PCI détecté) et charge les modules ;
   * recharge udev (`udevadm control --reload-rules && udevadm trigger`) ;
   * note ce qu'il a créé dans `/var/lib/tuxrgb/installed.list` (et
     sauvegarde un éventuel fichier existant).
3. **Terminé** : OpenRGB redémarre et détecte vos périphériques. Option
   « Lancer TuxRGB à l'ouverture de session » (activée par défaut) : au
   démarrage, TuxRGB s'installe directement dans la zone de notification et
   réapplique votre éclairage.

## Fenêtre, démarrage et sauvegarde

* Lancé depuis le menu : fenêtre **maximisée**. Lancé à l'ouverture de
  session (`--minimized`) : **zone de notification** uniquement.
* « Réduire » garde TuxRGB dans la **barre des tâches** (et dans la zone de
  notification) ; le bouton ✕ le range dans la zone de notification. Lancée
  directement, l'AppImage crée son entrée de menu et son icône dans
  `~/.local/share` pour que la barre des tâches affiche la bonne icône ; une
  nouvelle version remplace l'ancienne icône à toutes les tailles (32 à
  256 px) et rafraîchit les caches d'icônes de KDE / GTK.
* **Changer d'icône** : `python3 scripts/set-icon.py mon-icone.png` (ou .svg)
  régénère toutes les tailles (menu, fenêtre, zone de notification de 22 à
  128 px, logo HiDPI), puis reconstruire l'AppImage.
* **Instance unique** : relancer TuxRGB alors qu'il tourne déjà (menu, raccourci,
  zone de notification) ramène la fenêtre existante au premier plan
  (nom D-Bus `io.github.tuxrgb.TuxRGB`) – jamais deux moteurs OpenRGB.
* **Tous les réglages** (effet, couleurs, synchronisation avec le bureau,
  palette, luminosité, saturation, disposition avec rotation / miroir, tailles
  des connecteurs ARGB, langue, apparence, comportement) sont dans
  `~/.config/tuxrgb/config.json` : écrits une seconde après chaque
  changement, à la fermeture de la fenêtre et à la déconnexion / l'extinction,
  puis réappliqués dès le lancement (fondu d'entrée). Un ancien `config.toml`
  est converti automatiquement, et la saturation par effet des versions 1.x
  devient la saturation globale.
* **Extinction propre** : TuxRGB prend un inhibiteur « delay » de logind ;
  à `PrepareForShutdown` / `PrepareForSleep`, toutes les LED passent au noir
  immédiatement (sans fondu) – chaque contrôleur reçoit le noir en même
  temps, puis une deuxième fois dès la première confirmée –, et l'arrêt
  continue au plus tard 400 ms après (délai strict, le système n'est jamais
  ralenti). À la fermeture de session (SIGTERM), même chose ; le serveur
  OpenRGB bloque SIGTERM pour survivre le temps d'envoyer le noir. Au réveil,
  l'éclairage revient en fondu.

« Plus tard » laisse TuxRGB fonctionner en accès limité. Tout est réversible :
**Réglages → Accès matériel → Réinitialiser les permissions / Accès matériel**
retire exactement ce qui a été installé, restaure l'ancien fichier, efface
l'état du premier lancement et relance l'assistant (utile après un changement
de carte mère, de RAM ou de périphérique). Même chose en ligne de commande :
`tuxrgb --reset-hardware`.

Sécurité du programme d'aide qui configure l'accès matériel, et protections de la RAM : [SECURITE.md](SECURITE.md).
