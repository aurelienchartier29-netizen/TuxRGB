# Sécurité du programme d'aide (`tuxrgb-helper`)

[← Retour au README](../README.md)

* Seul composant exécuté en root, via Polkit (`pkexec`), le temps d'une
  action. Il n'accepte que des verbes fixes (`install`, `enable-smbus`,
  `enable-fans`, `reset`, `status`), sans aucun autre argument ; aucune
  commande ne passe par un shell.
* Ventilation : `enable-fans` copie le programme d'aide dans
  `/usr/local/libexec/tuxrgb/` et installe `tuxrgb-fans.service`, qu'il
  lance en root (`fan-daemon`). Le service n'écoute que sur
  `/run/tuxrgb/fans.sock`, qui n'appartient qu'à l'utilisateur qui l'a
  activé (0600) ; chaque ligne reçue est vérifiée (identifiants, bornes,
  courbes croissantes) et il n'écrit que dans les fichiers `pwmN` /
  `pwmN_enable` des ventilateurs qu'il a trouvés lui-même. L'état d'origine
  de chaque ventilateur est noté avant toute modification et remis par
  `ExecStopPost=… fan-restore` dès que le service s'arrête, plante ou que le
  système s'éteint. « Réinitialiser les permissions » arrête et supprime
  tout.
* Environnement effacé, `PATH` réduit aux dossiers système (pas de
  `/usr/local`), `umask 022` ; un outil (`udevadm`, `modprobe`,
  `rpm-ostree`, `grubby`…) n'est lancé que s'il appartient à root et n'est
  modifiable par personne d'autre.
* Paramètre noyau : une seule valeur possible, `acpi_enforce_resources=lax`,
  vérifiée par liste blanche et jeu de caractères strict avant tout outil ou
  fichier (testé contre `; rm -rf /`, `$(id)`, `init=/bin/sh`…). La méthode
  est choisie selon le système : `rpm-ostree kargs` (Bazzite, Fedora Atomic),
  `grubby` (Fedora, RHEL, Nobara), `kernelstub` (Pop!_OS),
  `/etc/default/grub` + `update-grub` / `grub-mkconfig` / `grub2-mkconfig`
  (Debian, Ubuntu, Mint, Arch, openSUSE), entrées systemd-boot
  (`/boot/loader/entries/*.conf` + `/etc/kernel/cmdline`), rEFInd
  (`refind_linux.conf`) ; NixOS et openSUSE transactionnel reçoivent les
  instructions exactes. **Rien n'est touché si le contrôleur SMBus a déjà
  son pilote** (accès RAM fonctionnel) ; chaque fichier modifié est
  sauvegardé dans `/var/lib/tuxrgb/boot-backup/` et noté pour
  « Réinitialiser ».
* Règles udev reçues : taille et longueur de ligne bornées, ASCII, pas de
  caractères de contrôle, clés et affectations sur liste blanche.
* Écritures atomiques sans suivre de lien symbolique (`O_EXCL`) ; copie
  temporaire du programme d'aide (AppImage) sous un nom unique, en 0500,
  vérifiée puis supprimée après usage ; l'UID demandeur est journalisé.
