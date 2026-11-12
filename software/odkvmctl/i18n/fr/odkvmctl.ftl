# OpenDynKVM : odkvmctl et opendyn-kvm, français. Un message absent ici s'affiche en anglais.

## Marques et noms. Les noms du produit et des fonctions sont des termes : les changer ici suffit.
## -wireless est la fonction Bluetooth. Tant que le produit n'est pas listé auprès du Bluetooth SIG, elle s'appelle
## « sans fil » ; ensuite, mettre « Bluetooth » dans les deux formes. Elle suit le nom (« hôte sans fil »).

-product = OpenDynKVM
-wireless = { $case ->
   *[title] Sans fil
    [lower] sans fil
}
## Là où un téléphone ou un PC associe un clavier : le menu de l'appareil, qui garde son propre nom.
-host-pairing-settings = réglages Bluetooth
-hop = { $case ->
   *[title] Saut d'écran
    [lower] saut d'écran
}

product-name = { -product }

## Mots fréquents

yes = oui
no = non
on = activé
off = désactivé

## Erreurs renvoyées par le KVM

err-unknown = requête inconnue
err-malformed = requête mal formée
err-locked = désactivé sur le KVM (le réactiver avec `odkvmctl config set software-config on` et un appui sur la touche du KVM, ou maintenir la touche à la mise sous tension du KVM)
err-busy = occupé, réessayez
err-unsupported = non pris en charge par ce firmware
err-invalid-value = valeur invalide
err-needs-key-press = appuyez sur la touche du KVM pour confirmer
err-no-device = aucun appareil sur ce port
err-not-active = seul le PC actif peut faire cela : basculez d'abord le KVM sur ce PC

## Apprendre un raccourci

learn-learned = appris
learn-timed-out = aucune touche formant un raccourci n'a été pressée à temps
learn-cancelled = arrêté (le KVM a changé de PC, ou l'apprentissage a été annulé)
hotkey-warn-twin = C'est aussi le raccourci « { $name } » : un appui ferait les deux.
hotkey-warn-alone = { $key } seule basculerait, et les PC ne la recevraient plus.

## Désactiver le contrôle par logiciel

lock-warn-switching = Aucun PC ne pourra plus basculer le KVM par logiciel : ni avec odkvmctl, ni avec l'application ou son icône de la zone de notification. La touche du KVM et les raccourcis clavier continuent de fonctionner.
lock-warn-config = Aucun PC ne pourra plus modifier de réglage par logiciel : ni modes de port, ni profils, ni raccourcis, ni mises à jour du firmware ou redémarrages, et Vial passe en lecture seule. L'état reste visible.
lock-warn-back-on = Le réactiver passe par la touche du KVM : un appui dans les { $seconds } s suivant la demande, ou la touche maintenue à la mise sous tension du KVM.

## Liaison avec un PC

link-unpowered = éteint
link-detached = non connecté
link-enumerating = connexion
link-connected = connecté
link-suspended = en veille
link-down = liaison coupée

## Ports en façade

port-state-empty = vide
port-state-probing = détection
port-state-hosted = actif
port-state-passthrough = transmission directe
port-state-unpowered = non alimenté
port-state-fault = défaut
port-state-unsupported = non pris en charge

mode-auto = Automatique
mode-normalized = Normalisé
mode-mirrored = Mis en miroir
mode-mirrored-identity = Miroir + identité
mode-passthrough = Transmission directe (suit le PC actif)
mode-passthrough-pc1 = Transmission directe, fixée sur PC1
mode-passthrough-pc2 = Transmission directe, fixée sur PC2
mode-short-auto = Auto
mode-short-normalized = Normalisé
mode-short-mirrored = Miroir
mode-short-identity = Identité
mode-short-passthrough = Direct
mode-short-pinned-pc1 = Fixé PC1
mode-short-pinned-pc2 = Fixé PC2

reason-not-auto = choisi par vous
reason-fits-normalized = toutes les entrées tiennent dans les interfaces normalisées
reason-extra-inputs = a des entrées que les interfaces normalisées ne portent pas
reason-other-port-hid = appareil HID sur le port Autre
reason-non-hid = a des interfaces autres que HID
reason-xinput = manette XInput
reason-endpoint-budget = plus assez de points de terminaison pour le mettre en miroir
reason-identity-taken = un autre port utilise déjà l'identité

speed-low = low-speed
speed-full = full-speed
speed-high = high-speed

class-keyboard = clavier
class-mouse = souris
class-consumer = touches multimédia
class-gamepad = manette
class-xinput = XInput
class-vendor-hid = entrées du fabricant
class-vendor-channel = commandes du fabricant
class-other-hid = autre HID
class-audio = audio
class-video = vidéo
class-storage = stockage
class-hub = hub

## La touche du KVM

key-action-none = Rien
key-action-toggle-pc = Passer à l'autre PC
key-action-next-profile = Profil suivant
key-action-other-port = Faire défiler le mode du port Autre
key-action-next-target = Cible suivante (les PC, puis les hôtes { -wireless(case: "lower") })

## Alimentation

adv-none = aucun
adv-default = USB par défaut
adv-1a5 = 1,5 A
adv-3a = 3 A
power-source-pwr = port PWR
power-source-pc = USB du PC

## Mise à jour du firmware

fw-state-idle = en attente
fw-state-receiving = envoi
fw-state-awaiting-key = appuyez sur la touche du KVM pour installer
fw-state-writing = installation
fw-state-done = terminé
fw-state-failed = échec

## Le module sans fil

bt-module-up = en marche
bt-module-starting = en démarrage
bt-module-programming = en cours de mise à jour
bt-module-failed = ne fonctionne pas avec son firmware : { -wireless(case: "lower") } coupé jusqu'à la prochaine mise sous tension
bt-module-gave-up = abandonné après 5 essais : { -wireless(case: "lower") } coupé
bt-module-absent = non monté

## Autotest

selftest-hub0 = Hub-0 (ports en façade) introuvable
selftest-link1 = pas de liaison avec la puce de PC1
selftest-link2 = pas de liaison avec la puce de PC2
selftest-hub1 = le hub de PC1 ne fonctionne pas
selftest-hub2 = le hub de PC2 ne fonctionne pas
selftest-port-kb = interrupteur d'alimentation du port clavier
selftest-port-ms = interrupteur d'alimentation du port souris
selftest-port-ot = interrupteur d'alimentation du port Autre
selftest-power = mesures du port PWR invraisemblables
selftest-at32-fw1 = la programmation de la puce de PC1 a échoué (nouvel essai à la prochaine mise sous tension)
selftest-at32-fw2 = la programmation de la puce de PC2 a échoué (nouvel essai à la prochaine mise sous tension)
selftest-at32-gave-up1 = programmation de la puce de PC1 abandonnée après 5 essais
selftest-at32-gave-up2 = programmation de la puce de PC2 abandonnée après 5 essais
selftest-at32-xtal1 = le quartz de la puce de PC1 n'oscille pas (pas d'USB vers PC1)
selftest-at32-xtal2 = le quartz de la puce de PC2 n'oscille pas (pas d'USB vers PC2)
selftest-bt-fw = le module { -wireless(case: "lower") } ne fonctionne pas avec son firmware (nouvel essai à la prochaine mise sous tension)
selftest-bt-gave-up = programmation du module { -wireless(case: "lower") } abandonnée après 5 essais
selftest-running = en cours
selftest-passed = réussi
selftest-running-so-far = en cours, jusqu'ici : { $failed }
selftest-failed = ÉCHEC : { $failed }

## Pourquoi le KVM a basculé

switch-source-key = touche du KVM
switch-source-hotkey = raccourci
switch-source-software = logiciel
switch-source-boot = démarrage
switch-source-power = le PC actif s'est éteint

## Diagnostic

diag-group-system = Système
diag-group-clocks = Horloges
diag-group-usb = USB
diag-group-power = Alimentation
diag-group-pc-chips = Puces côté PC
diag-group-wireless = { -wireless }
diag-group-other = Autres

diag-uptime = Temps de fonctionnement
diag-reset-cause = Dernier redémarrage
diag-rp-temp = Température du RP2354B
diag-selftest = Autotest
diag-dropped = Sorties du moteur perdues
diag-fw-updates = Mises à jour du firmware par USB
diag-xosc = Quartz (XOSC)
diag-pll-sys = PLL système
diag-pll-usb = PLL USB
diag-rosc = Oscillateur en anneau
diag-at-clock = Horloge de l'AT32F405 nᵒ { $n }
diag-hub0-speed = Liaison du Hub-0
diag-hub0-up = Hub-0 énuméré
diag-phy-swap = Inversion D+/D− (errata E1)
diag-kb-speed = Appareil du port clavier
diag-ms-speed = Appareil du port souris
diag-ot-speed = Appareil du port Autre
diag-pc-speed = Vitesse USB de { $pc }
diag-pc-link = Liaison de { $pc }
diag-pc-resets = Réinitialisations du bus de { $pc }
diag-link-errors = Erreurs SPI de l'AT32F405 nᵒ { $n }
diag-hub0-errors = Changements du Hub-0 non traités
diag-enum-fails = Énumérations ratées sur les ports avant
diag-vbus = VBUS { $port }
diag-cc = CC { $port }
diag-at-3v3 = 3,3 V à l'AT32F405 nᵒ { $n }
diag-budget = Budget des ports en façade
diag-kb-faults = Défauts du port clavier
diag-ms-faults = Défauts du port souris
diag-ot-faults = Défauts du port Autre
diag-at-temp = Température de l'AT32F405 nᵒ { $n }
diag-at-fw = Firmware de l'AT32F405 nᵒ { $n }
diag-bt-module = Module ESP32-C3
diag-bt-fw = Firmware de l'ESP32-C3
diag-bt-link-errors = Erreurs UART de l'ESP32-C3
diag-bt-hosts = Hôtes { -wireless(case: "lower") } connectés
diag-unknown = élément { $id }

reset-software = redémarrage par logiciel
reset-power-on = mise sous tension
reset-brown-out = sous-tension (l'alimentation a chuté)
reset-glitch = perturbation de l'alimentation
reset-run-pin = broche RUN
reset-watchdog = chien de garde (aussi après une mise à jour)
reset-debugger = débogueur
reset-other = cause { $cause }
hz-stopped = 0 Hz (arrêté)
clock-crystal = quartz 12 MHz
clock-internal = oscillateur interne : pas d'USB
swap-found = { $state }, détecté en fonctionnement
fw-updates-on = activées, firmware dans la partition { $partition }
fw-updates-no-table = désactivées : pas de table de partitions (programmé par une sonde de débogage)
fw-updates-read-failed = désactivées : la ROM de démarrage n'a pas rendu la table de partitions ({ $detail })
fw-updates-other-table = désactivées : autre table de partitions ({ $detail })
fw-updates-other-layout = désactivées : les partitions diffèrent de celles de ce firmware ({ $detail })
fw-updates-unknown-partition = désactivées : partition en cours inconnue ({ $detail })
state-other = état { $state }

diag-help-reset-cause = Orange après une sous-tension ou une perturbation : les 5 V ont chuté. Les redémarrages voulus n'ont pas de couleur.
diag-help-rp-temp = Orange dès 70 °C, rouge dès 85 °C.
diag-help-selftest = Rouge quand une partie du KVM ne fonctionne pas ; orange pour l'interrupteur d'alimentation d'un port en façade.
diag-help-dropped = Travail pour lequel le firmware n'avait pas de place. Orange si différent de 0.
diag-help-fw-updates = Si le KVM accepte les mises à jour du firmware par un port PC. Orange sinon : le code entre parenthèses dit ce que sa vérification a lu, utile pour signaler le problème. Pas de couleur sur une carte programmée par une sonde de débogage.
diag-help-xosc = Le quartz 12 MHz ; rouge s'il n'oscille pas.
diag-help-clk-sys = Mesurée par rapport au quartz. Rouge à 1 % d'écart, ou si elle ne dépasse pas clk_usb de 10 % (RP2350-E12).
diag-help-clk-usb = Mesurée par rapport au quartz. Rouge à plus de 0,25 % d'écart de 48 MHz (USB full-speed).
diag-help-clk-adc = Rouge à plus de 1 % d'écart de 48 MHz.
diag-help-at-clock = Rouge quand la puce tourne sans son quartz : elle n'a alors pas d'USB vers son PC.
diag-help-hub0-speed = Le hub des ports en façade. Rouge en low-speed : D+ et D− sont croisés pour le réglage du PHY (errata E1).
diag-help-phy-swap = Si l'USB du RP2354B inverse D+ et D− (errata E1) : activé pour les cartes 0.8 à 0.12.
diag-help-pc-speed = Orange en full-speed : l'interrogation à 125 µs demande le high-speed. Essayez un autre câble ou un autre port.
diag-help-pc-link = Rouge quand la puce côté PC ne répond pas ; orange tant que le PC ne l'a pas configurée.
diag-help-link-errors = Échanges ratés avec la puce côté PC depuis le démarrage, une fois qu'elle a répondu pour la première fois.
diag-help-hub0-errors = Changements de port signalés par le Hub-0 que l'hôte USB du KVM ne traite pas, effacés pour qu'ils ne masquent pas les autres ports. Quelques-uns au plus ; davantage mérite un signalement.
diag-help-enum-fails = Appareils des ports avant que le KVM n'a pas pu lire, chacun réessayé quelques fois en coupant et rétablissant le port. Débrancher un appareil pendant sa lecture compte aussi.
diag-help-vbus = Bon de 4,75 à 5,50 V ; orange de 4,40 à 4,75 V (trop bas) ou jusqu'à 5,70 V ; rouge au-delà. Pas de couleur sous 0,8 V : rien ne l'alimente.
diag-help-cc = Rouge au-dessus de 2,3 V (5 V sur CC : câble ou contact). Orange sous 0,2 V avec 5 V présents : un câble sans Rp.
diag-help-3v3 = Bon de 3,135 à 3,465 V (±5 %), contre une référence à ±3,3 % ; orange jusqu'à 3,0 / 3,6 V.
diag-help-budget = Orange quand les appareils branchés pourraient consommer plus que ce que la source offre : utilisez le port PWR.
diag-help-faults = Coupures pour surintensité de l'interrupteur d'alimentation du port.
diag-help-at-temp = Orange dès 85 °C, rouge dès 105 °C. Jusqu'à 20 °C d'écart d'une puce à l'autre.
diag-help-at-fw = Orange s'il diffère de celui du RP2354B : en cours de reprogrammation, ou la programmation a échoué.
diag-help-bt-module = Orange pendant que le KVM le programme (après une mise à jour, 10 à 20 s). Rouge s'il ne fonctionne pas avec son firmware à ce démarrage (nouvel essai à la prochaine mise sous tension), ou après 5 échecs de suite : le KVM ne réessaie alors que si sa touche est maintenue à la mise sous tension, ou avec Firmware → Réessayer.
diag-help-bt-fw = Le module fonctionne avec le firmware que le KVM embarque : une mise à jour du KVM le met aussi à jour.
diag-help-bt-link-errors = Échanges ratés avec le module depuis le démarrage, sa programmation comprise.
diag-help-bt-hosts = Hôtes associés, connectés en ce moment et prêts à recevoir des saisies.

## Réglages en ligne de commande

parse-colour = couleur '{ $value }' : utilisez #rrggbb
parse-hotkey = raccourci '{ $value }' : { $error } (essayez double:scrolllock ou ctrl+alt+1)
parse-host-name = nom '{ $value }' : au plus { $max } octets (vide : Hôte n)
parse-profile-name = nom '{ $value }' : de 1 à { $max } octets
parse-mode = mode '{ $value }' : { $modes }
parse-profile-modes = '{ $value }' : trois modes pour clavier, souris et autre, p. ex. auto,auto,passthrough
parse-number = '{ $value }' : un nombre jusqu'à { $max }
parse-one-of = '{ $value }' : utilisez { $choices }
parse-unknown-setting = réglage inconnu

## La fenêtre : barre latérale et en-tête

window-title-sim = { -product } (simulé)
simulated-kvm = KVM simulé
page-overview = Vue d'ensemble
page-ports = Ports
page-displays = Écrans
page-keyboard = Clavier
page-settings = Réglages
page-firmware = Firmware
page-wireless = { -wireless }
light-mode = Mode clair
dark-mode = Mode sombre
no-kvm-connected = Aucun KVM connecté
leds = LED
active-pc = PC actif
profile = Profil
connected-to = Connecté · { $device }
reading = Lecture…
not-connected = Non connecté

## Vue d'ensemble

chip-active = ACTIF
chip-this-computer = cet ordinateur
pc-vbus = VBUS { $volts } V · offre { $offers }
showing = Affiché
switch-here = Basculer ici
front-ports = Ports en façade
unnamed-device = (appareil sans nom)
host-has-input = { $host } reçoit le clavier et la souris en { -wireless(case: "lower") }. Les ports en transmission directe restent sur { $pc }.
power = Alimentation
power-left = Par { $source }, environ { $milliamps } mA restants pour les ports en façade
budget-low = Les appareils branchés pourraient demander plus : utilisez le port PWR.
selftest-line = Autotest { $result }
recent = Récent

## Ports

port-keyboard = Port clavier
port-mouse = Port souris
port-other = Port Autre
port-short-keyboard = Clavier
port-short-mouse = Souris
port-short-other = Autre
redetect = Redétecter
mode = Mode
no-device = Aucun appareil
running-mode = Fonctionne en { $mode }
up-to-ma = jusqu'à { $milliamps } mA
over-current-faults = { $count ->
    [one] { $count } défaut de surintensité
   *[other] { $count } défauts de surintensité
}
profiles = Profils
profiles-note = Chaque profil est un ensemble de modes de port, p. ex. des appareils émulés pour le travail et la transmission directe pour les jeux. Un appui long sur la touche du KVM choisit le suivant (page Clavier). Les ports ci-dessous suivent le profil actif.
name = Nom
profiles-in-use = Profils utilisés :
remember-profile = Démarrer dans le dernier profil actif (sinon dans le profil 0, et choisir un profil n'écrit rien dans la mémoire flash du KVM)
config-locked = La configuration par logiciel est désactivée sur le KVM.
config-locked-how = Pour la réactiver : Réglages → Contrôle par logiciel, puis appuyez sur la touche du KVM.

## Clavier

hotkeys = Raccourcis
hotkey-toggle = Changer de PC
hotkey-pc1 = Choisir PC1
hotkey-pc2 = Choisir PC2
hotkey-next = Cible suivante
cant-learn-firmware = Le firmware de ce KVM ne sait pas apprendre de raccourcis : tapez-le plutôt
cant-learn-not-active = Seul le PC actif peut apprendre un raccourci : basculez d'abord le KVM sur ce PC
cant-learn-no-keyboard = Aucun clavier que le KVM lit n'est branché
learning-another = Un autre raccourci est en cours d'apprentissage
learned-with-warning = { $warning } Appliquer l'utilise quand même.
learned = Appris depuis le clavier. Appliquer l'utilise.
not-learned = Non appris : { $why }.
kvm-no-answer = Le KVM n'a pas répondu.
learn-hint-sim = Ou appuyez sur Apprendre, puis sur le raccourci dans cette fenêtre : il joue le clavier du KVM simulé.
learn-hint = Ou appuyez sur Apprendre, puis sur le raccourci au clavier du KVM.
press-the-hotkey = Appuyez sur le raccourci… { $seconds } s
apply = Appliquer
cancel = Annuler
learn = Apprendre
learn-hover = Appuyer sur le raccourci au clavier du KVM au lieu de le taper
passthrough-keyboard-note = Un clavier en transmission directe va droit au PC : le KVM ne le voit pas, donc aucun raccourci n'y fonctionne et aucun n'en est appris.
double-tap-window = ms de fenêtre pour le double appui
kvm-key = La touche du KVM
tap = Appui
long-press = Appui long
press-tap = appui
press-long-from = appui long dès { $seconds } s
held-for-long-press = Maintien pour un appui long :
kvm-key-note = Quel que soit ce réglage, un appui confirme une mise à jour du firmware en attente, et en veille le premier appui ne fait que réveiller le KVM.
keymap-vial = Disposition (Vial)
keymap-note = Modifiez la disposition avec l'application Vial tant que son interface est affichée. La disposition fonctionne aussi interface désactivée. Changer l'interface fait redétecter le KVM par les deux PC (environ 1 s).
vial-interface = Interface Vial :
vial-off = Désactivée
vial-auto = Automatique (si aucun appareil en miroir n'a besoin du point de terminaison)
vial-always = Toujours
apply-keymap = Appliquer la disposition aux claviers que le KVM normalise

## Réglages

this-computer = Cet ordinateur
autostart = Démarrer avec le système, dans la zone de notification
could-not-change = Impossible de le modifier : { $error }
appearance = Apparence :
theme-system = Système
theme-dark = Sombre
theme-light = Clair
language = Langue :
language-system = Système
brightness = luminosité
pc-colour = Couleur de { $pc }
pc-side = Côté PC
polling = Interrogation :
polling-125us = 125 µs (latence minimale)
polling-1ms = 1 ms (pour les BIOS capricieux)
power-cycle = Couper brièvement l'alimentation des appareils en transmission directe quand ils changent de PC
standby = Quand les PC sont éteints ou en veille
standby-note = Quand aucun PC n'a utilisé le KVM depuis 10 s, ses LED s'éteignent et les ports en façade sont coupés. Un PC qui s'allume, ou un appui sur la touche du KVM, le réveille.
auto-select = Suivre le PC allumé : quand le PC actif s'éteint, l'autre devient actif
standby-wake = Garder clavier et souris alimentés pendant la veille d'un PC, pour qu'une touche le réveille
standby-glow = Laisser la touche luire en veille (sinon toutes les LED sont éteintes)
save-now = Enregistrer maintenant
factory-settings = Réglages d'usine
unsaved-changes = Les changements non enregistrés sont écrits dans quelques secondes.
software-control = Contrôle par logiciel
software-control-note = Chaque PC peut les désactiver. Les réactiver passe par la touche du KVM : un appui dans les { $seconds } s suivant la demande faite ici, ou la touche maintenue à la mise sous tension du KVM.
allow-switching = Autoriser le basculement par logiciel
allow-config = Autoriser la configuration par logiciel
not-turned-on = Non activé : la touche du KVM n'a pas été pressée à temps.
press-key-to-confirm = Appuyez sur la touche du KVM pour confirmer (encore { $seconds } s). Sa LED pulse en blanc.
confirm-lock-switching = Désactiver le basculement par logiciel ?
confirm-lock-config = Désactiver la configuration par logiciel ?
turn-off = Désactiver

## Firmware

versions = Versions
wireless-module = Module { -wireless(case: "lower") }
hardware = Matériel
serial = Numéro de série
uptime = Temps de fonctionnement
gave-up-note-chip = Le KVM a cessé d'essayer après 5 échecs de suite, pour ne pas reprogrammer la puce à chaque mise sous tension. Il réessaie avec un nouveau firmware, quand sa touche est maintenue à la mise sous tension, ou maintenant :
try-again = Réessayer (le KVM redémarre)
update = Mise à jour
image-file = Fichier image :
install = Installer
press-key-to-install = Appuyez sur la touche du KVM pour installer la mise à jour.
installed = Installé ; le KVM redémarre.
fw-file-title = Fichier du firmware
fw-file-hint = le dossier d'une version, kvm-rp2354.bin ou un .uf2
browse = Parcourir…
fw-shipped = Firmware fourni
fw-shipped-note = fourni avec cette application
fw-file-is = Firmware { $version } pour le matériel { $major }.x
fw-same = Le KVM a déjà cette version.
fw-newer = Plus récent que le { $kvm } du KVM.
fw-older = Plus ancien que le { $kvm } du KVM : l'installer revient en arrière.
fw-no-file = Aucun firmware choisi : choisissez un fichier ou le dossier d'une version.
update-over-pc-note = Par le port de cet ordinateur, vers le KVM en marche. Le nouveau firmware démarre à l'essai : s'il ne démarre pas, le KVM revient à l'ancien.
pick-firmware = Choisir le firmware du KVM
firmware-files = Firmware du KVM
boot-title = Dans le chargeur d'amorçage (port PWR/SERVICE)
boot-kvm = { -product } avec le firmware { $version } ; une mise à jour va dans la partition { $slot }.
boot-kvm-empty = { -product } sans firmware ; il va dans la partition { $slot }.
boot-new = Une nouvelle carte, encore vide. La préparer écrit la table des partitions et le firmware.
boot-new-unpartitioned = Firmware { $version }, mais pas de table des partitions (programmé par une sonde de débogage). La préparer écrit les deux.
boot-other = Pas un { -product } : un RP2350 avec une autre table des partitions ou un autre firmware. Rien n'y est installé.
boot-unknown-note = Sans cela, cette application ne distingue pas un { -product } d'une nouvelle carte : n'installez que sur un { -product } qui a un firmware, et ne préparez qu'une nouvelle carte.
boot-drive = Lecteur : { $path }
boot-no-drive-note = Son lecteur n'est pas monté : l'installation le monte, ou ouvrez-le d'abord dans le gestionnaire de fichiers.
boot-set-up = Préparer
boot-writes = Écrit { $file } : firmware { $version } pour le matériel { $major }.x
boot-writing = écriture
boot-restart = Redémarrer sans installer
boot-restart-hover = Quitte le chargeur d'amorçage : la carte démarre le firmware qu'elle a.
boot-installed = Installé : la carte redémarre avec le nouveau firmware. Branchez-la aux PC pour l'utiliser.
boot-none-title = Installer par le port PWR/SERVICE
boot-none-note = Pour une nouvelle carte, ou une dont le firmware ne démarre pas : branchez cet ordinateur sur le port PWR/SERVICE du KVM. Une nouvelle carte apparaît ici d'elle-même ; une autre si l'on maintient son bouton à trou en la branchant.
boot-found-hint = Un { -product } attend dans son chargeur d'amorçage sur le port PWR/SERVICE.
open-firmware-page = Page Firmware
identify = Identifier (faire clignoter les LED)
diagnostics = Diagnostic
measuring = Mesure…
diag-bad = { $count ->
    [one] { $count } mauvais
   *[other] { $count } mauvais
}
diag-warnings = { $count ->
    [one] { $count } avertissement
   *[other] { $count } avertissements
}
diag-all-good = tout va bien
diag-readings = { $count } mesures, il y a { $seconds } s
copy = Copier
copy-hover = Copier les mesures en texte
looking-for-kvm = Recherche du KVM…
plug-in-kvm = Branchez le KVM sur cet ordinateur.
udev-hint = Installé depuis les sources ? Installez alors software/odkvmctl/udev/70-opendyn-kvm.rules (DEVELOPMENT.md).

## Hôtes sans fil (la fonction Bluetooth ; son nom est le terme -wireless)

host-connecting = connexion…
host-hover = { -wireless }, { $state }
send = Envoyer
cancel-pairing = Annuler l'association
host-pairing = Un hôte { -wireless(case: "lower") } s'associe
passkey-text = L'hôte affiche six chiffres : tapez-les au clavier du KVM et appuyez sur Entrée (Échap annule), ou saisissez-les ici. D'ici là, le clavier du KVM n'écrit nulle part ailleurs.
module-is = Le module est { $state }.
module-programming-note = Après une mise à jour du firmware, le KVM programme le module avec le firmware qu'il embarque, ce qui prend 10 à 20 s. Les hôtes { -wireless(case: "lower") } se reconnectent ensuite.
module-failed-note = Le { -wireless(case: "lower") } reste coupé jusqu'à la prochaine mise sous tension du KVM, qui réessaie alors une fois. Les deux PC fonctionnent comme avant.
gave-up-note-module = Le KVM a cessé d'essayer après 5 échecs de suite, pour ne pas reprogrammer le module à chaque mise sous tension. Il réessaie avec un nouveau firmware, quand sa touche est maintenue à la mise sous tension, ou maintenant :
pair-new-host = Associer un nouvel hôte
pair-note = Rend le KVM visible sous le nom « { -product } » pendant une minute. Associez-le ensuite depuis le PC, la tablette ou le téléphone, dans ses { -host-pairing-settings }, comme un clavier. Si l'hôte affiche six chiffres, tapez-les au clavier du KVM et appuyez sur Entrée.
pair = Associer
slots-full = Oubliez d'abord un hôte : les quatre emplacements sont pris
discoverable = Visible · { $seconds } s
stop = Arrêter
hosts = Hôtes
hosts-note = Les hôtes { -wireless(case: "lower") } reçoivent le clavier et la souris propres au KVM (miroir, identité et transmission directe restent aux PC). « Rester connecté » rend le basculement instantané ; « Seulement si choisi » convient aux téléphones et tablettes qui masquent leur clavier à l'écran quand un clavier est connecté.
slot-free = libre
active = actif
forget = Oublier
forget-hover = Supprime l'association. Pour associer à nouveau, supprimez aussi « { -product } » sur l'hôte.
link-every = Toutes les { $ms } ms
link-may-skip = { $count ->
    [one] peut en sauter { $count }
   *[other] peut en sauter { $count }
}
link-signal = signal { $dbm } dBm
policy-stay = Rester connecté
policy-on-select = Seulement si choisi
a-host = un hôte { -wireless(case: "lower") }

## Zone de notification

tray-not-connected = { -product } : non connecté
tray-host-active = { -product } : { $host } actif ({ -wireless(case: "lower") })
tray-pc-active = { -product } : { $pc } actif
tray-switch-to = Basculer vers { $target }
tray-open = Ouvrir
tray-quit = Quitter

## Événements récents

ev-switched = Basculé vers { $target } ({ $source })
ev-port-changed = { $port } modifié
ev-fault-port = { $port } : { $fault }
ev-fault = Défaut : { $fault }
ev-setting-changed = Réglage { $name } modifié
ev-hotkey-learned = Raccourci appris : { $hotkey }
ev-hotkey-not-learned = Apprentissage d'un raccourci : { $why }
ev-pairing-open = Association { -wireless(case: "lower") } ouverte
ev-pairing-closed = Association { -wireless(case: "lower") } fermée
ev-passkey-wanted = Un hôte { -wireless(case: "lower") } demande son code
ev-paired = Associé : { $host } (emplacement { $slot } ; nommez-le dans la page { -wireless })
ev-forgot = Hôte { -wireless(case: "lower") } { $host } oublié
ev-host-connected = { $host } connecté
ev-host-disconnected = { $host } déconnecté
ev-switched-host = Basculé vers { $host } ({ -wireless(case: "lower") })
ev-monitor-not-switched = Écran non basculé : { $error }
fault-over-current = surintensité
fault-enumeration = énumération échouée
fault-link-down = liaison coupée
fault-power-budget = alimentation insuffisante
fault-hub = erreur de hub
no-diagnostics = Le firmware de ce KVM n'a pas encore de diagnostic : mettez-le à jour.
kvm-refused = Le KVM a refusé : { $why }.

## Écrans qui suivent le KVM (DDC/CI)

profile-n = Profil { $n }
display-follow = Suit le KVM
display-pc1 = Reste sur PC1
display-pc2 = Reste sur PC2
display-off = Non basculé
input-none-pc = — non basculé
input-none-host = — reste sur le PC
display-switched = { $monitor } basculé sur { $input }
displays-title = Les entrées des écrans suivent le KVM
displays-note = Quand le KVM change de PC (ou passe à un hôte { -wireless(case: "lower") }) ou de profil, cet ordinateur bascule les écrans ci-dessous (DDC/CI) : dans chaque profil, un écran peut suivre le KVM, rester sur un PC, ou être laissé tel quel. Le KVM signale chaque changement aux deux PC : configurez donc chaque écran sur un seul PC si possible.
displays-two-pcs = Les deux PC ne partagent pas ces réglages : un écran configuré sur les deux reçoit chaque commande deux fois, et ne suit aucune des configurations si elles diffèrent. Un seul PC suffit si l'écran écoute aussi sur une entrée qu'il n'affiche pas : « Afficher PC1 » / « Afficher PC2 » vers l'autre PC puis retour le montre. S'il ne revient pas, configurez-le de la même façon sur les deux PC.
displays-ddc-on = Le DDC/CI doit être activé dans le menu de l'écran.
look-again = Rechercher à nouveau les écrans
asking-monitors = Interrogation des écrans…
this-computer-is = Cet ordinateur est { $pc }
kvm-not-connected = KVM non connecté
no-monitor = Aucun écran n'a répondu. Vérifiez que le DDC/CI est activé dans le menu de l'écran.
saved-not-connected = Enregistré, non connecté en ce moment
input-other = autre
input-other-hover = Une entrée que l'écran ne liste pas, en hexadécimal (p. ex. 0x0F), puis Entrée
use-current = Utiliser l'actuelle
use-current-hover = L'entrée que cet écran affiche en ce moment
show-target = Afficher { $target }
monitor-shows = affiche { $input }
not-answering = ne répond pas
target-input = Entrée de { $target }
displays-hosts-note = Les hôtes { -wireless(case: "lower") } sont facultatifs : tant que l'un d'eux a le clavier et la souris, un écran qui suit le KVM l'affiche s'il a une entrée ici, et reste sur le PC sinon.
in-each-profile = Dans chaque profil :
input-missing = Un profil a besoin d'une entrée qui n'est pas choisie ci-dessus.
could-not-save = Impossible d'enregistrer les réglages de cet ordinateur : { $error }
input-unknown = Entrée { $value }
ddc-no-answer = pas de réponse (le DDC/CI est-il activé dans le menu de l'écran ?)
ddc-garbled = réponse brouillée
ddc-unexpected = réponse inattendue
ddc-unsupported-code = l'écran ne le prend pas en charge
ddc-not-connected = { $id } : non connecté
ddc-unsupported-os = Le DDC/CI n'est pris en charge que sous Windows et Linux
ddc-no-access = { $path } : pas d'accès. Installez le paquet, ou la règle udev du README, puis reconnectez-vous.
ddc-no-i2c-dev = { $path } : absent, le module i2c-dev n'est pas chargé. Installez le paquet, ou chargez-le comme l'indique le README.

## Communication avec le KVM

error-timeout = le KVM n'a pas répondu
error-device = le KVM a refusé : { $why }
error-protocol = erreur de protocole : { $error }
protocol-unexpected = réponse inattendue { $answer }
protocol-update-ended = la mise à jour s'est terminée dans l'état { $state }
error-not-confirmed = la touche du KVM n'a pas été pressée à temps : rien n'a changé
host-n = Hôte { $n }
hid-open-linux = { $error } (installez software/odkvmctl/udev/70-opendyn-kvm.rules pour un accès sans root)
no-kvm-found = aucun { -product } trouvé

## Fichiers de firmware

img-damaged-uf2 = fichier UF2 endommagé
img-uf2-partition-table = cet UF2 contient aussi la table des partitions (kvm-first.uf2) ; utilisez kvm-rp2354.uf2 ou kvm-rp2354.bin
img-uf2-other-chip = UF2 pour une autre puce (famille { $family })
img-uf2-out-of-range = bloc UF2 à { $address } hors limites
img-too-big = { $kb } Ko, c'est plus que ce que contient une partition de firmware ({ $max } Ko)
img-not-rp2354b = pas un firmware RP2354B (pas de définition d'image RP2350)
img-wrong-kind = image RP2350 du mauvais type (drapeaux { $flags })
img-no-fw-id = pas un firmware { -product } (pas d'identifiant de firmware)
img-other-hardware = le firmware { $version } est pour le matériel { $major }.x, ce KVM est le matériel { $hw }
img-not-first = pas un UF2 de première installation (kvm-first.uf2 : table des partitions et firmware)
fw-read = { $file } : { $error }
fw-not-in-folder = pas de { $name } dans { $folder }
boot-no-picoboot = la ROM de démarrage n'a pas d'interface PICOBOOT
boot-no-permission = pas d'autorisation pour la ROM de démarrage du RP2350 (USB 2e8a:000f) : installez la règle udev (software/odkvmctl/udev/70-opendyn-kvm.rules)
boot-no-access = impossible d'ouvrir la ROM de démarrage du RP2350 : { $error }
boot-usb = USB : { $error }
boot-rom-ignored = la ROM de démarrage a ignoré le fichier (pas pour les partitions de cette carte)
boot-rom-locked = un autre programme tient la ROM de démarrage (picotool ?)
boot-rom-bad-address = la ROM de démarrage s'est arrêtée : une adresse hors de la flash
boot-rom-write-error = la ROM de démarrage n'a pas pu écrire la flash
boot-rom-reboot-failure = la ROM de démarrage a écrit le firmware mais n'a pas pu le démarrer
boot-no-drive = le lecteur de la ROM de démarrage (RP2350) n'est pas monté : ouvrez-le dans le gestionnaire de fichiers, puis réinstallez
boot-write = écriture sur le lecteur de la ROM de démarrage : { $error }
boot-no-restart = la carte n'a pas redémarré après la copie (rien pendant 20 s)

## odkvmctl : aide

cli-about = Piloter le commutateur { -product }
cli-sim = Parler à un KVM simulé intégré au lieu du vrai
cli-status = Afficher l'état du KVM (par défaut)
cli-diag = Mesurer les horloges, tensions, températures et liaisons USB du KVM, chacune notée bon, avertissement ou mauvais ; code de sortie 1 si quelque chose est mauvais
cli-list = Lister les interfaces de contrôle de KVM que cet ordinateur voit
cli-watch = Afficher les événements (basculements, changements de port, défauts) au fil de l'eau
cli-switch = Basculer le clavier, la souris et l'autre appareil vers un PC, ou (avec le module { -wireless(case: "lower") }) vers un hôte { -wireless(case: "lower") }
cli-wireless = Hôtes { -wireless(case: "lower") } (KVM avec le module) : les lister (par défaut), en associer un nouveau, en oublier, en nommer un, ou régler ce qui arrive à un hôte pendant qu'une autre cible est active
cli-port = Afficher un port en façade, régler son mode, ou redétecter son appareil
cli-profile = Lister les profils, en activer un (profile next, profile 1, profile gaming), ou en modifier un (profile 1 name gaming, profile 1 mouse passthrough)
cli-profile-which = Le numéro ou le nom d'un profil, ou next
cli-profile-setting = name, ou un port : keyboard, mouse, other
cli-hotkey = Afficher ou régler un raccourci : double:scrolllock, double:rctrl:noswallow, ctrl+alt+1, none ; ou l'apprendre (learn) des prochaines touches pressées au clavier du KVM
cli-config = Lire et modifier les réglages
cli-identify = Faire clignoter les LED du KVM
cli-update = Installer un firmware RP2354B : sur une carte qui attend dans son chargeur d'amorçage sur le port PWR/SERVICE s'il y en a une, sinon par un port PC (demande un appui sur la touche du KVM)
cli-reboot = Redémarrer le KVM, ou mettre une puce dans son chargeur d'amorçage
cli-display = Les écrans dont cet ordinateur peut basculer l'entrée en DDC/CI (sans KVM)
cli-display-list = Les écrans, l'entrée affichée par chacun et les entrées qu'il propose (par défaut ; prend quelques secondes)
cli-display-input = Basculer un écran sur une entrée, p. ex. display input XYZ-1A2B-1234 0x11 (identifiants de display list)
cli-hosts-list = Les quatre emplacements d'hôtes : nom, état, comportement
cli-hosts-pair = Rendre le KVM visible pendant 60 s sous le nom « { -product } » ; l'associer ensuite sur le PC, la tablette ou le téléphone. Si l'hôte affiche six chiffres, les taper au clavier du KVM et appuyer sur Entrée (ou utiliser passkey)
cli-hosts-pair-stop = stop ferme l'association plus tôt
cli-hosts-passkey = Les six chiffres qu'affiche un hôte en cours d'association, quand le clavier du KVM n'est pas à portée
cli-hosts-forget = Oublier l'association d'un hôte : emplacement 1-4, ou all
cli-hosts-name = Nommer un emplacement d'hôte (vide : « Hôte n »)
cli-hosts-policy = stay : l'hôte reste connecté et y basculer est instantané. on-select : connecté seulement quand il est choisi (pour les téléphones et tablettes qui masquent leur clavier à l'écran quand un clavier est connecté)
cli-port-redetect = Redétecter l'appareil (nécessaire après avoir changé l'appareil d'un port en transmission directe)
cli-config-get = Afficher un réglage, ou tous
cli-config-set = Modifier un réglage (config get donne les noms et les valeurs actuelles)
cli-config-save = Écrire les réglages dans la mémoire flash du KVM maintenant (sinon 5 s après le dernier changement)
cli-config-reset = Revenir aux réglages d'usine (les verrous du logiciel sont conservés)
cli-target-next = La cible suivante : PC1, PC2, puis chaque hôte { -wireless(case: "lower") } connecté
cli-reboot-at32-retry = Donner aux AT32F405 jusqu'à 5 essais de programmation de plus, puis redémarrer
cli-reboot-wireless-retry = Donner au module { -wireless(case: "lower") } jusqu'à 5 essais de programmation de plus, puis redémarrer

## odkvmctl : sortie

cli-port-auto = auto : { $why }
cli-port-set-to = réglé sur { $mode } : { $why }
cli-status-head = { -product } ({ $device }) : firmware { $firmware }, AT32 { $at1 } / { $at2 }, matériel { $hardware }, numéro de série { $serial }
cli-module-firmware = Module { -wireless(case: "lower") } : firmware { $version }
cli-module-state = Module { -wireless(case: "lower") } : { $state }
cli-active-host = Actif : hôte { -wireless(case: "lower") } { $slot } « { $host } » (PC de départ { $pc })
cli-active = Actif : { $target }
cli-this-computer = cet ordinateur est { $pc }
cli-profile-line = Profil : { $n } ({ $name })
cli-host-slot = Hôte { $slot }
cli-pairing-open = l'association { -wireless(case: "lower") } est ouverte (encore { $seconds } s)
cli-passkey-waits = un hôte en cours d'association attend son code : tapez-le au clavier du KVM, puis Entrée
cli-budget-low = les appareils pourraient demander plus
cli-power = Alimentation : { $power }
cli-selftest = Autotest : { $result }
cli-try-again = réessayer : { $command }, ou maintenir la touche du KVM à la mise sous tension
cli-ports = Ports :
cli-switching-disabled = le basculement par logiciel est désactivé
cli-config-disabled = la configuration par logiciel est désactivée
cli-locked = Verrouillé sur l'appareil : { $what }
cli-host-ready = connecté, { $ms } ms
cli-profiles-in-use = { $count } sur { $max } utilisés : odkvmctl config set profiles <1-{ $max }>
cli-no-profile = pas de profil '{ $which }' : un nombre de 0 à { $last } ou un nom
cli-learn-sim = le clavier du KVM simulé ne se presse pas d'ici : essayez Apprendre dans `opendyn-kvm --sim`
cli-learn-press = Appuyez sur le nouveau raccourci au clavier du KVM dans les { $seconds } s. Les touches n'atteignent aucun PC pendant ce temps.
cli-learn-unsupported = le firmware de ce KVM ne sait pas apprendre de raccourcis : donnez le raccourci en texte, p. ex. ctrl+alt+1
cli-nothing-changed-why = { $why } : rien n'a changé
cli-use-it-for = L'utiliser pour { $name } ? [o/N]
cli-turn-it-off = Le désactiver ? [o/N]
## Mots, en plus de y et yes, qui répondent oui à une question [o/N], séparés par des virgules.
cli-yes-words = o, oui
cli-readings = { $count } mesures : { $verdict }
cli-ev-back-on-pc = saisie revenue sur un PC
cli-ev-host-active = basculé vers l'hôte { -wireless(case: "lower") } { $slot }
cli-ev-pairing-changed = association { -wireless(case: "lower") } modifiée
cli-ev-host-changed = hôte { -wireless(case: "lower") } { $slot } modifié
cli-no-monitors = aucun écran trouvé
cli-inputs = entrées : { $inputs }
cli-input-number = entrée '{ $value }' : un nombre comme 0x11 ou 17
cli-no-interface = aucune interface de contrôle { -product } trouvée
cli-connecting = connexion au KVM
cli-watching = observation de { $device } (Ctrl+C pour arrêter)
cli-no-module = ce KVM n'a pas de module { -wireless(case: "lower") }
cli-no-host-in-slot = aucun hôte n'est associé à cet emplacement
cli-no-module-boards = ce KVM n'a pas de module { -wireless(case: "lower") } (les cartes à partir de 0.16.0 peuvent en avoir un)
cli-pair-usage = '{ $value }' : utilisez `wireless pair` ou `wireless pair stop`
cli-pairing-opened = L'association est ouverte pendant { $seconds } s : associez « { -product } » sur le PC, la tablette ou le téléphone.
cli-pairing-digits = S'il affiche six chiffres, tapez-les au clavier du KVM et appuyez sur Entrée.
cli-six-digits = les six chiffres qu'affiche l'hôte
cli-no-passkey-wanted = aucun hôte n'attend de code
cli-slot-or-all = emplacement 1-4 ou all
cli-slot = emplacement 1-4
cli-no-profiles = ce firmware n'a pas de profils
cli-profile-not-in-use = le profil { $n } n'est pas utilisé : odkvmctl config set profiles { $count }
cli-missing-value = profil { $setting } : la valeur manque
cli-nothing-changed = Rien n'a changé.
cli-unknown-setting = réglage inconnu '{ $name }'
cli-press-key-within = Appuyez sur la touche du KVM dans les { $seconds } s pour confirmer…
cli-saved = enregistré
cli-factory-restored = réglages d'usine rétablis
cli-reading = lecture de { $file }
cli-firmware-for = firmware { $version } pour le matériel { $major }.x
cli-update-file = Le firmware : le dossier d'une version, kvm-rp2354.bin ou un .uf2 (par défaut : le firmware fourni avec ce programme)
cli-boot-no-drive = lecteur non monté
cli-no-firmware = aucun firmware indiqué, et aucun n'est fourni avec ce programme
cli-no-kvm-anywhere = aucun { -product } sur un port PC, ni dans son chargeur d'amorçage sur le port PWR/SERVICE
cli-boot-several = plusieurs cartes dans leur chargeur d'amorçage : branchez-en une à la fois
cli-boot-unknown = Sans cela, odkvmctl ne distingue pas un KVM d'une nouvelle carte. Copiez vous-même kvm-rp2354.uf2 (un KVM avec firmware) ou kvm-first.uf2 (une nouvelle carte) sur son lecteur RP2350.

## Saut d'écran (le nom de la fonction est le terme -hop)

page-screens = { -hop }
switch-source-edge = le pointeur a franchi un bord d'écran
hop-line = { -hop } : { $state }
hop-state-off = désactivé dans ce profil
hop-state-active = activé
hop-state-no-absolute = nécessite le pointeur absolu
hop-state-not-normalized = nécessite le port souris en mode Normalisé
hop-state-no-screens = ce PC n'a aucun écran dans la disposition
hop-state-boot-protocol = désactivé tant que le PC est dans son setup firmware
hop-state-wireless = désactivé tant qu'un hôte { -wireless(case: "lower") } a la main
accel-flat = Linéaire (vitesse seule)
accel-precision = Courbe de précision, comme sous Windows
accel-adaptive = Adaptative, comme sous Linux
mapping-desktop = Sur tous ses écrans (Linux)
mapping-main-screen = Sur son écran principal seulement (Windows)
parse-screen = '{ $value }' : écrivez un écran sous la forme pc1 2560x1440 150% at 1920,0 desk 0,0 (mise à l'échelle et positions facultatives), ou off
parse-range = '{ $value }' : un nombre de { $min } à { $max }
screens-none-found = aucun écran trouvé
screens-xrandr-missing = impossible de lancer xrandr pour lister les écrans ({ $error })
screens-xrandr-failed = xrandr n'a pas pu lister les écrans : { $error }
screens-unsupported-os = la détection des écrans n'est prise en charge que sous Windows et Linux
profile-hop = Saut
profile-hop-hover = { -hop } dans ce profil (sa propre page)
hop-firmware-too-old = Le firmware de ce KVM n'a pas le { -hop(case: "lower") } : mettez-le à jour.
hop-what = Le KVM déplace lui-même le pointeur sur les écrans disposés ci-dessous. Poussez-le au-delà d'un bord vers l'écran de l'autre PC, et le KVM bascule sur ce PC, clavier compris.
hop-turn-on-absolute = Activer le pointeur absolu
hop-mouse-normalized = Passer le port souris en Normalisé dans ce profil
hop-in-profiles = Activé dans ces profils
hop-profile-active = { $name } (actif)
hop-absolute-pointer = Pointeur absolu sur la souris du KVM
hop-absolute-pointer-note = Le { -hop(case: "lower") } en a besoin. L'activer ou le désactiver fait reconnecter une fois les appareils du KVM sur les deux PC.
hop-arrangement = Disposition
hop-arrangement-note = Faites glisser les écrans à leur place. Le pointeur passe là où deux écrans se touchent et saute l'espace entre eux ; un bord sans rien à côté retient le pointeur.
hop-overlap = Certains écrans se chevauchent : le pointeur ne peut pas passer de l'un à l'autre.
hop-add-screen = Ajouter un écran { $pc }
hop-detect = Détecter les écrans de { $pc }
hop-detect-hover = Les écrans de cet ordinateur, avec leur résolution, leur mise à l'échelle et leur place dans ses réglages d'affichage
hop-remove-screen = Retirer
hop-mapping-of = { $pc } place le pointeur
hop-mapping-note = Windows place un pointeur absolu sur son écran principal seulement. Le KVM atteint ses autres écrans en y déplaçant le curseur comme une souris, et Windows accélère ce mouvement : désactivez « Améliorer la précision du pointeur » sur un tel PC pour des bords exacts, ou laissez { -product } tourner dessus.
hop-mapping-windows = Cet ordinateur tourne sous Windows, qui place le pointeur absolu sur son écran principal seulement : sur tous les écrans, le pointeur arrive au mauvais endroit et les autres écrans restent hors d'atteinte. Choisissez « Sur son écran principal seulement (Windows) ».
hop-no-screens = Pas encore d'écran : ajoutez-en un pour chaque PC.
hop-main-screen = principal
hop-screen-hover = Cliquez pour modifier, faites glisser pour déplacer
hop-screen-pc = Ordinateur
hop-screen-resolution = Résolution
hop-screen-scale = Mise à l'échelle
hop-screen-desk = Position sur son ordinateur
hop-screen-at = Position ici
hop-screen-desk-note = L'emplacement de l'écran dans les réglages d'affichage de son ordinateur, en pixels de cet ordinateur. L'écran principal est en 0, 0.
hop-detected = { $count ->
    [one] { $pc } a maintenant 1 écran, comme cet ordinateur.
   *[other] { $pc } a maintenant { $count } écrans, comme cet ordinateur.
} { $pc } place le pointeur : { $mapping }.
hop-pointer = Pointeur
hop-accel = Accélération
hop-speed = Vitesse
hop-resistance = Résistance du bord
hop-corner = Coins
hop-pointer-note = Les PC n'accélèrent pas un pointeur absolu, alors le KVM s'en charge. La résistance du bord est la distance dont il faut pousser le pointeur contre un bord avant qu'il passe à l'autre PC ; les coins sont la distance depuis chaque bout de ce bord où il ne passe jamais, pour que les coins restent faciles à atteindre. Pendant un glisser, il ne passe jamais.
cli-screens = Saut d'écran : la disposition des écrans des deux PC et le mouvement du pointeur
cli-screens-list = Afficher la disposition et les réglages du pointeur
cli-screens-detect = Mettre les écrans de cet ordinateur dans la disposition
cli-screens-which-pc = le KVM n'indique pas quel PC est cet ordinateur (simulateur ou port de service) : réglez plutôt les écrans avec odkvmctl config set screen0 …
cli-hop-mapping = { $pc } place le pointeur : { $mapping }
cli-hop-pointer = Pointeur : { $accel }, vitesse { $speed }, résistance du bord { $resistance } px, coins { $corner } px
