# Sécurité

[← Retour au README](../README.md)

* [Protection de la RAM et du matériel](#protection-de-la-ram-et-du-matériel)
* [Programme d'aide Linux (`tuxrgb-helper`)](#programme-daide-linux-tuxrgb-helper)
* [Windows : mode administrateur](#windows--mode-administrateur)
* [Mises à jour](#mises-à-jour)
* [Limites connues](#limites-connues)

## Protection de la RAM et du matériel

Allumer une barrette de RAM, c'est écrire sur le **SMBus**, le bus qui relie
aussi les puces SPD des barrettes (leur « carte d'identité », lue par le BIOS
au démarrage). Les barrettes abîmées signalées par la communauté OpenRGB
viennent presque toujours de **deux programmes qui parlaient au bus en même
temps** : chaque écriture se fait en plusieurs étapes (choisir un registre,
puis l'écrire), et deux séquences entremêlées peuvent envoyer une valeur au
mauvais endroit. TuxRGB (3.5.1) empile plusieurs protections :

* **Désactivé par défaut** sur une nouvelle installation, en deux
  interrupteurs séparés (Réglages → Accès matériel, et l'assistant) : la
  **RAM**, et les **cartes mères pilotées par le SMBus** (ASRock Polychrome,
  anciennes ASUS Aura et Gigabyte RGB Fusion, EVGA). Les cartes mères
  récentes, en USB, n'en ont pas besoin. Désactivé, aucun détecteur SMBus
  n'est lancé : rien n'est lu ni écrit sur ce bus.
* **Jamais deux programmes à la fois** : tant qu'un autre logiciel RGB est
  ouvert (OpenRGB, liquidctl, i2c-tools sous Linux ; OpenRGB, iCUE,
  Armoury Crate / Aura, MSI Center, Mystic Light, RGB Fusion, SignalRGB sous
  Windows), TuxRGB démarre son moteur **sans** la RAM ni les cartes mères
  SMBus, l'indique par un bandeau, puis les reprend tout seul dès que l'autre
  programme se ferme (vérifié toutes les 5 s).
* **Verrou partagé du bus** toujours activé (`shared_smbus_access`) : sous
  Windows, OpenRGB attend son tour derrière HWiNFO, AIDA64 et les autres
  outils de surveillance qui prennent le même verrou global.
* **30 écritures par seconde au plus** sur un appareil SMBus / I2C (RAM,
  cartes mères et cartes graphiques SMBus) : le bus reste libre la plupart du
  temps, pour le firmware et la lecture des SPD. Les animations restent
  fluides.
* « Débloquer l'accès SMBus » (`acpi_enforce_resources=lax`) n'est proposé
  que si la RAM est activée et que le firmware réserve le bus, et il
  l'explique : ce réglage autorise Linux à partager un bus que le firmware
  peut aussi utiliser. En DDR5, le pilote de sonde `spd5118` est écarté pour
  qu'il ne lise pas les puces SPD pendant qu'OpenRGB écrit.
* Ventilateurs : une courbe n'arrête jamais un ventilateur sans point à
  0 % voulu ; tous passent à 100 % dès 90 °C (processeur ou carte
  graphique), quelle que soit la courbe ; une sonde illisible plus de 3 s
  rend le ventilateur au contrôle automatique de la carte mère ; leur état
  d'origine est remis à l'arrêt du service (Linux) ou de TuxRGB (Windows).

## Programme d'aide Linux (`tuxrgb-helper`)

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

## Windows : mode administrateur

Le mode administrateur (nécessaire pour la RAM et les cartes mères SMBus)
démarre TuxRGB avec des droits élevés à chaque ouverture de session, par une
tâche planifiée. Depuis la 3.5.1, aucun programme de l'utilisateur ne peut
en profiter pour lancer son propre code avec ces droits :

* Le mode administrateur n'est accepté que pour un TuxRGB **installé dans
  Program Files** (dossier modifiable seulement par un administrateur), pas
  pour une copie portable dans Téléchargements.
* Avec ces droits, le moteur OpenRGB n'est lancé **que depuis Program
  Files** : la variable `TUXRGB_OPENRGB`, `%LOCALAPPDATA%` et le `PATH` sont
  ignorés. Les variables Qt qui choisissent des modules à charger
  (`QT_PLUGIN_PATH`…) sont retirées, et le moteur démarre dans son propre
  dossier.
* La définition de la tâche est écrite dans un nouveau fichier, puis
  verrouillée en lecture seule pendant que `schtasks` la lit ; le rapport de
  l'action administrateur n'est jamais écrit à travers un fichier existant.
* L'état d'origine des ventilateurs est rangé dans le dossier
  d'installation (supprimé par la désinstallation).
* Les variables de développement (`TUXRGB_DEV_*`) sont ignorées avec ces
  droits.

## Mises à jour

* Téléchargées en HTTPS depuis les versions publiées sur GitHub, et
  **refusées sans somme de contrôle SHA-256** publiée à côté (3.5.1 : avant,
  un fichier sans somme était accepté).
* Windows : l'installateur vérifié est gardé ouvert en lecture seule, puis
  revérifié juste avant d'être lancé : il ne peut pas être remplacé entre la
  vérification et l'installation.

## Limites connues

* Le moteur OpenRGB écoute sur `127.0.0.1` (port 6742 et suivants) : un
  autre compte de la même machine peut lui envoyer des couleurs. Aucun
  accès au réseau, mais à garder en tête sur un PC partagé.
* Un programme lancé sous votre compte peut, comme vous, régler vos
  ventilateurs (la règle des 90 °C reste appliquée par le service) : c'est
  le même niveau de confiance que vos autres fichiers.
* La sécurité de la RAM dépend aussi du matériel : sur DDR5 et sur les
  cartes mères qui réservent le SMBus, laissez la RAM désactivée si vous
  n'avez pas de barrettes RGB. Ne lancez jamais deux logiciels RGB en
  même temps (TuxRGB le vérifie pour les plus courants, pas pour tous).
