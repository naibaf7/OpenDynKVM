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
err-not-allowed = cet ordinateur ne peut pas encore piloter le KVM par la liaison { -wireless(case: "lower") } : demandez avec `odkvmctl wireless allow` (ou dans la fenêtre), puis appuyez sur la touche du KVM

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
port-state-module = module { -wireless(case: "lower") }

mode-auto = Automatique
mode-normalized = Normalisé
mode-mirrored = Mis en miroir
mode-mirrored-identity = Miroir + identité
mode-passthrough = Transmission directe

reason-not-auto = choisi par vous
reason-fits-normalized = toutes les entrées tiennent dans les interfaces normalisées
reason-extra-inputs = a des entrées que les interfaces normalisées ne portent pas
reason-aux-port-hid = appareil HID sur un port Aux
reason-non-hid = a des interfaces autres que HID
reason-xinput = manette XInput
reason-endpoint-budget = plus assez de points de terminaison pour le mettre en miroir
reason-identity-taken = un autre port utilise déjà l'identité
reason-needs-passthrough = il lui faudrait la transmission directe, que le port Aux 1 n'a pas

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
key-action-aux2-port = Faire défiler le mode du port Aux 2
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
bt-module-needs-update = doit être mis à jour : { -wireless(case: "lower") } coupé d'ici là
bt-module-absent = non monté

## Autotest

selftest-hub0 = Hub-0 (ports en façade) introuvable
selftest-link1 = pas de liaison avec la puce de PC1
selftest-link2 = pas de liaison avec la puce de PC2
selftest-hub1 = le hub de PC1 ne fonctionne pas
selftest-hub2 = le hub de PC2 ne fonctionne pas
selftest-port-kb = interrupteur d'alimentation du port clavier
selftest-port-ms = interrupteur d'alimentation du port souris
selftest-port-a1 = interrupteur d'alimentation du port Aux 1
selftest-port-a2 = interrupteur d'alimentation du port Aux 2
selftest-power = mesures du port PWR invraisemblables
selftest-at32-fw1 = la programmation de la puce de PC1 a échoué (nouvel essai à la prochaine mise sous tension)
selftest-at32-fw2 = la programmation de la puce de PC2 a échoué (nouvel essai à la prochaine mise sous tension)
selftest-at32-gave-up1 = programmation de la puce de PC1 abandonnée après 5 essais
selftest-at32-gave-up2 = programmation de la puce de PC2 abandonnée après 5 essais
selftest-at32-xtal1 = le quartz de la puce de PC1 n'oscille pas (pas d'USB vers PC1)
selftest-at32-xtal2 = le quartz de la puce de PC2 n'oscille pas (pas d'USB vers PC2)
selftest-bt-fw = le module { -wireless(case: "lower") } n'a pas la version de ce firmware : il doit être mis à jour
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
diag-a1-speed = Appareil du port Aux 1
diag-a2-speed = Appareil du port Aux 2
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
diag-supply = 5 V au plus bas dans la dernière minute
diag-supply-dips = Creux des 5 V sous 4,4 V
diag-dropouts = Coupures des 5 V de { $pc }
diag-kb-faults = Défauts du port clavier
diag-ms-faults = Défauts du port souris
diag-a1-faults = Défauts du port Aux 1
diag-a2-faults = Défauts du port Aux 2
diag-at-temp = Température de l'AT32F405 nᵒ { $n }
diag-at-fw = Firmware de l'AT32F405 nᵒ { $n }
diag-bt-module = Module ESP32
diag-bt-fw = Firmware de l'ESP32
diag-bt-link-errors = Erreurs UART de l'ESP32
diag-bt-hosts = Hôtes { -wireless(case: "lower") } connectés
diag-bt-settings-held = Hôtes { -wireless(case: "lower") } aux réglages gardés en RAM
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
diag-help-budget = Ce que la source annonce, moins la consommation propre du KVM : une estimation, pour information. Si la source suit, cela se voit aux 5 V au plus bas.
diag-help-supply = Les 5 V dont le KVM s'alimente (le port PWR, ou le plus élevé des PC), au plus bas dans la dernière minute. Vert tant qu'ils tiennent ; orange s'ils restent sous 4,5 V pendant 2 s ; rouge sous 4,4 V pendant une demi-seconde : utilisez le port PWR.
diag-help-supply-dips = Combien de fois depuis le démarrage les 5 V ont brièvement plongé sous 4,4 V (l'appel de courant d'un appareil qu'on branche, par exemple). Un compte, pas un défaut.
diag-help-dropouts = Combien de fois depuis le démarrage les 5 V de ce PC ont disparu moins de 3 s : un port qui a coupé sous la charge, ou le PC qui redémarre.
diag-help-faults = Coupures pour surintensité de l'interrupteur d'alimentation du port.
diag-help-at-temp = Orange dès 85 °C, rouge dès 105 °C. Jusqu'à 20 °C d'écart d'une puce à l'autre.
diag-help-at-fw = Orange s'il diffère de celui du RP2354B : en cours de reprogrammation, ou la programmation a échoué.
diag-help-bt-module = Orange pendant sa programmation (après une mise à jour, environ 20 s), et tant qu'il a un autre firmware que le KVM : le { -wireless(case: "lower") } reste coupé jusqu'à ce que Firmware → Mettre à jour le module { -wireless(case: "lower") }, ou odkvmctl update --module, lui envoie celui du KVM.
diag-help-bt-fw = Le module fonctionne avec le firmware que le KVM embarque : une mise à jour du KVM le met aussi à jour.
diag-help-bt-link-errors = Échanges ratés avec le module depuis le démarrage, sa programmation comprise.
diag-help-bt-hosts = Hôtes associés, connectés en ce moment et prêts à recevoir des saisies.
diag-help-bt-settings-held = Orange : un hôte change ses réglages de notification si souvent que le module les garde en RAM jusqu'à son redémarrage, pour ménager sa flash. Tout continue de fonctionner ; ils sont écrits à la prochaine association. Si cela revient, supprimez l'hôte et associez-le à nouveau.

## Réglages en ligne de commande

parse-colour = couleur '{ $value }' : utilisez #rrggbb
parse-hotkey = raccourci '{ $value }' : { $error } (essayez double:scrolllock ou ctrl+alt+1)
parse-host-name = nom '{ $value }' : au plus { $max } octets (vide : Hôte n)
parse-profile-name = nom '{ $value }' : de 1 à { $max } octets
parse-mode = mode '{ $value }' : { $modes }
parse-profile-modes = '{ $value }' : huit modes, pour les ports clavier, souris, Aux 1 et Aux 2 et les appareils { -wireless(case: "lower") } 1–4 (jamais passthrough sur Aux 1), p. ex. auto,auto,auto,passthrough,auto,auto,auto,auto
parse-number = '{ $value }' : un nombre jusqu'à { $max }
parse-one-of = '{ $value }' : utilisez { $choices }
parse-keycode = '{ $value }' : pas un keycode (une touche, ctrl+c, mouse1, pad5, dpad-up, mo(1), lt(1,space), mt(shift,a), none, trans ou un nombre)
parse-control = '{ $value }' : pas une commande (une touche, mouse1–mouse16, pad1–pad32, hat-up ou une direction d'axe comme x- ou wheel+)
parse-macro-step = '{ $value }' : pas une étape de macro (text:…, tap:<keycode>, down:<keycode>, up:<keycode>, delay:<ms>)
parse-unknown-setting = réglage inconnu

## La fenêtre : barre latérale et en-tête

window-title-sim = { -product } (simulé)
simulated-kvm = KVM simulé
page-overview = Vue d'ensemble
page-ports = Ports
page-displays = Écrans
page-keyboard = Clavier
page-keymaps = Keymaps
keymaps = Keymaps
keymaps-note = Quatre keymaps de huit couches chacune : ce que fait chaque touche, chaque bouton de souris et de manette, et chaque axe. Chaque profil en utilise une, ou aucune (alors chaque appareil fait ce qui est écrit). Une keymap ne change que les appareils en mode Normalisé ; l'application Vial modifie la partie clavier de la keymap du profil actif.
keymap-layer-title = Keymap { $keymap }, couche { $layer }
map-keyboard = Clavier
map-mouse = Souris
map-gamepad = Manette
map-learn = Apprendre
map-learn-hover = Appuyez sur une commande d'un appareil pour la choisir ici. Pendant ce temps, les commandes n'atteignent aucun hôte.
map-learn-select = Appuyez sur une commande de n'importe quel appareil pour la choisir ({ $seconds } s)
map-learn-assign = Appuyez sur ce que { $control } doit faire, sur n'importe quel appareil ({ $seconds } s)
map-learn-held = Maintenu : { $controls }
map-learn-none-held = Maintenu : rien
map-learn-escape = Maintenez Échap 3 s pour arrêter. Pendant ce temps, les commandes n'atteignent aucun hôte.
map-learn-escape-held = Gardez Échap enfoncée : l'apprentissage s'arrête dans { $seconds } s. Relâchée avant, Échap est la commande pressée.
map-learn-stop = Arrêter l'apprentissage
map-clear = Effacer
map-keymap-unused = Le profil actif ({ $profile }) utilise { $uses } : ce que vous changez ici n'a d'effet que lorsqu'un profil utilise cette keymap.
map-keymap-no = aucune keymap
map-keymap-n = la keymap { $n }
map-use-here = Utiliser cette keymap dans { $profile }
map-not-normalized = Les keymaps ne changent que les appareils en mode Normalisé. Ne passent pas par elles maintenant : { $devices }.
map-normalize = Mettre { $device } en Normalisé
map-not-active = Seul le PC qui reçoit les commandes affiche celles maintenues, les apprend et calibre : basculez d'abord le KVM sur ce PC.
map-control = Commande : { $control }
map-axes = Axes
map-keymap = Keymap
map-layer = Couche
map-used-by = Utilisée par
map-used-by-hover = Les appareils normalisés du profil passent par cette keymap
map-virtual-gamepad = Manette virtuelle pour les PC
map-gamepad-off = Désactivée
map-gamepad-hid = Manette HID
map-virtual-gamepad-note = Ce que les keymaps envoient à une manette, et les manettes en mode Normalisé, atteignent les PC comme cette manette. Désactivée tant qu'aucun jeu n'en a besoin : une manette que personne n'utilise peut dérouter les jeux. La changer fait redétecter le KVM.
map-other-keys = Touches non dessinées
map-more-buttons = Boutons 6–16
map-axis-buttons = Mouvement et molette comme boutons (quand une règle d'axe en fait des boutons)
map-extra-buttons = Autres boutons : nommez-les comme vous voulez (gardé sur cet ordinateur)
map-name-hint = nom
map-device = Appareil
map-no-device = aucun branché
map-calibration = Calibrage
map-calibrate = Calibrer
map-calibration-reset = Réinitialiser
map-calibrate-move = Amenez chaque stick et gâchette en butée, puis appuyez sur Terminé.
map-calibrate-done = Terminé
map-calibrate-let-go = Relâchez tout…
map-does = Fait : { $action }
map-as-it-says = Comme écrit
map-nothing = Rien
map-transparent = Transparent
map-transparent-hover = Ce qu'en font les couches du dessous
map-press-target = Appuyer sur ce qu'il doit faire
map-press-target-hover = La prochaine commande pressée, sur n'importe quel appareil, donne sa propre action à celle-ci
map-choose = Choisir…
map-group-mouse = Boutons et touches de souris
map-group-gamepad = Boutons de manette et croix directionnelle
map-group-layers = Couches : tant que maintenu (MO), bascule (TG), seule (TO), une fois (OSL), bascule par tapes (TT)
map-group-media = Média et système
map-axes-note = Chaque axe sur cette couche : où il va, mis à l'échelle, inversé, avec une zone morte. Un stick sur la souris la déplace à une vitesse, la souris sur un stick le pousse et le laisse revenir, un axe en boutons appuie sur ses deux directions.
map-axis = Axe
map-live = Maintenant
map-target = Va vers
map-scale = Échelle
map-invert = Inverser
map-deadzone = Zone morte
map-param = Vitesse
map-param-hover = Stick vers souris : pas par 8 ms à pleine déviation. Souris vers stick : vitesse du retour au centre (0 : il reste). En boutons : le seuil, en 256es.
map-sec-layers = Couches
map-sec-macros = Macros
map-sec-dances = Tap dances
map-sec-combos = Combinaisons
map-sec-overrides = Substitutions de touches
map-sec-repeats = Touches de répétition
map-sec-settings = Délais et options
map-sec-monitor = Moniteur
map-sec-hosts = Hôtes
map-macros-note = Seize macros, communes à toutes les keymaps, jouées par une commande réglée sur macro(n) : une étape par ligne, text:…, tap:…, down:… et up:… avec un keycode saisi comme ailleurs, delay:… en ms. Le texte est tapé selon la disposition du clavier de l'hôte qui le reçoit (Hôtes).
map-macro-too-long = Les macros ne tiennent pas dans leurs 2 Ko, ou une étape de texte contient un caractère hors ASCII.
map-discard = Abandonner
map-remove = Supprimer
map-dances-note = Une commande réglée sur td(n) fait une chose tapée, une autre maintenue, une autre tapée deux fois et une autre tapée puis maintenue, sur tout appareil : un bouton de manette aussi.
map-dance-tap = Tape
map-dance-hold = Maintien
map-dance-double = Double tape
map-dance-tap-hold = Tape puis maintien
map-term = Délai de tape
map-combos-note = Des commandes pressées ensemble font quelque chose à elles : jusqu'à quatre keycodes, tels qu'ils sont sur la couche (pad1 et pad2 pour deux boutons de manette).
map-combo-keys = Ensemble
map-combo-output = Fait
map-overrides-note = Avec ces modificateurs maintenus, une touche en envoie une autre : Maj+Retour arrière comme Suppr, par exemple.
map-ko-mods = Avec
map-ko-trigger = Touche
map-ko-replacement = Envoie
map-ko-negative = Pas avec
map-ko-suppress = Retenus
map-entry-on = Actif
map-repeats-note = Après la première touche, la touche Alternate Repeat envoie la seconde : après Gauche, Droite.
map-repeat-after = Après
map-repeat-alt = Alternative
map-repeat-mods = Modificateurs permis
map-settings-note = Communs à toutes les keymaps, comme les réglages QMK de Vial. Délais en millisecondes.
map-set-tapping-term = Délai de tape : plus court, c'est une tape, plus long un maintien
map-set-permissive-hold = Permissive hold : une autre touche tapée entre-temps en fait un maintien
map-set-hold-on-other = Maintien à l'appui d'une autre touche
map-set-retro-tapping = Retro tapping : maintenue seule puis relâchée, elle tape
map-set-chordal-hold = Chordal hold : une touche de la même main en fait une tape
map-set-quick-tap-term = Délai de tape rapide : repressée si tôt, elle répète sa tape
map-set-flow-tap-term = Délai de flow tap : pressée si tôt après une lettre, elle tape (0 : désactivé)
map-set-tapping-toggle = Tapes qui basculent une couche TT
map-set-tap-code-delay = Délai de tape des codes
map-set-caps-delay = Délai de Verr. Maj
map-set-combo-term = Délai de combinaison
map-set-oneshot-timeout = Délai one-shot (0 : aucun)
map-set-oneshot-tap-toggle = Tapes qui verrouillent une touche one-shot
map-set-auto-shift = Auto shift (bits)
map-set-auto-shift-timeout = Délai d'auto shift
map-set-grave-esc = Exceptions Grave Escape (bits)
map-set-magic = Réglages magic (bits)
map-set-mouse-delay = Touches souris : délai
map-set-mouse-interval = Touches souris : intervalle
map-set-mouse-delta = Touches souris : pas
map-set-mouse-max-speed = Touches souris : vitesse maximale
map-set-mouse-time-to-max = Touches souris : temps jusqu'à la vitesse maximale
map-set-wheel-delay = Touches molette : délai
map-set-wheel-interval = Touches molette : intervalle
map-set-wheel-max-speed = Touches molette : vitesse maximale
map-set-wheel-time-to-max = Touches molette : temps jusqu'à la vitesse maximale
map-monitor-note = Les rapports d'un appareil à mesure qu'ils arrivent : les bits déclarés par son descripteur sont teintés, ceux qui changent s'allument. Choisissez des bits (clic, puis Maj-clic) et faites-en un bouton ou un axe de manette : le pavé tactile d'une manette, par exemple, que son descripteur cache.
map-monitor-report = Interface { $iface }, rapport { $id } ({ $count } vus)
map-monitor-waiting = Utilisez l'appareil : ses rapports s'affichent ici.
map-fields = Surcharges de champs
map-fields-note = Gardées avec l'appareil. Un champ devient un bouton ou un axe de manette de l'appareil, que toute keymap peut alors mapper.
map-field-entry = Interface { $iface }, rapport { $id } : { $bits } bits à partir du bit { $bit } → { $target }
map-field-select = Choisissez des bits dans un rapport ci-dessus pour en faire une commande.
map-field-selected = Choisis : { $bits } bits à partir du bit { $first }
map-field-signed = Signé
map-field-invert = Inversé
map-field-make = En faire une commande
map-field-full = Un appareil a au plus { $max } surcharges de champs.
map-hosts-note = La disposition du clavier de chaque hôte : le texte des macros est tapé selon elle, et l'image du clavier montre ses légendes. Ce que chaque hôte se voit offrir, et en couleur ce qu'il a pris.
map-layout = Disposition du clavier
map-outputs = Sorties
map-outputs-none = Non configuré
map-outputs-note = Un hôte { -wireless(case: "lower") } prend ce à quoi il s'abonne ; celui qui ne prend pas de manette reçoit la keymap de repli du profil.
map-out-keyboard = Clavier
map-out-mouse = Souris
map-out-media = Touches multimédia
map-out-pointer = Pointeur absolu
map-out-gamepad = Manette
map-out-mirrors = Appareils en miroir
map-fallback = Là où la manette ne va pas
map-fallback-note = Un hôte qui ne prend pas de manette (un PC dont la manette virtuelle est désactivée, un hôte { -wireless(case: "lower") } qui ne s'y est pas abonné) reçoit cette keymap au lieu de celle du profil : les sticks vers la souris et les boutons vers des touches, par exemple.
map-fallback-keymap = Keymap de repli
map-keymap-none = aucune
map-group-kvm = Actions du KVM
map-physical = Clavier
map-physical-auto = Comme la disposition ({ $physical })
map-legends = Légendes : { $layout } ({ $host })
layout-us = Anglais (US)
layout-gb = Anglais (R.-U.)
layout-de = Allemand
layout-ch = Suisse allemand
layout-fr = Français
layout-es = Espagnol
layout-it = Italien
layout-pt = Portugais
layout-no = Norvégien
layout-se = Suédois / finnois
layout-dk = Danois
layout-jp = Japonais
layout-br = Portugais (Brésil)
pad-back = Retour
pad-start = Start
pad-home = Accueil
map-pad-look = Dessinée comme
map-pad-look-auto = Comme l'appareil : { $style }
pad-style-generic = Générique : boutons par numéro
pad-style-offset = Sticks décalés, boutons à lettres
pad-style-symmetric = Sticks côte à côte, formes, pavé tactile
pad-share = Partager
pad-options = Options
pad-touch-pad = Pavé tactile
pad-mute = Muet
pad-touch = Contact
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
power-supply = Par { $source } : { $volts } V, au plus bas { $lowest } V dans la dernière minute
power-declared = Les appareils en façade déclarent jusqu'à { $declared } mA (disponible estimé : { $milliamps } mA). La tension d'alimentation est stable.
supply-sags = Les 5 V fléchissent sous la charge : la source ou son câble ne suit pas tout à fait. Le port PWR aiderait.
supply-low = Les 5 V tombent sous 4,4 V : des appareils en façade peuvent décrocher. Utilisez le port PWR.
selftest-line = Autotest { $result }
recent = Récent

## Ports

port-keyboard = Port clavier
port-mouse = Port souris
port-aux1 = Port Aux 1
port-aux2 = Port Aux 2
redetect = Redétecter
mode = Mode
no-device = Aucun appareil
port-module-summary = Module { -wireless(case: "lower") }, pas pour les PC
port-module-note = Ce port porte le module { -wireless(case: "lower") } du KVM, une carte ESP32 : il sert les hôtes { -wireless(case: "lower") } et n'est relié à aucun PC. Débranchez la carte pour utiliser de nouveau le port pour un appareil. Pour atteindre depuis un PC une carte ESP32 à vous, réglez ce port sur un mode de transmission directe : une carte ESP32 va alors telle quelle au PC.
running-mode = Fonctionne en { $mode }
up-to-ma = jusqu'à { $milliamps } mA
over-current-faults = { $count ->
    [one] { $count } défaut de surintensité
   *[other] { $count } défauts de surintensité
}
profiles = Profils
profiles-note = Chaque profil règle le mode de chaque appareil et où vont ses entrées : vers l'hôte actif, ou fixées sur un hôte quel que soit l'actif. P. ex. des appareils émulés pour le travail et la transmission directe pour les jeux. Un appui long sur la touche du KVM choisit le suivant (page Clavier) ; l'actif est en gras.
name = Nom
profiles-in-use = Profils utilisés :
remember-profile = Démarrer dans le dernier profil actif (sinon dans le profil 0, et choisir un profil n'écrit rien dans la mémoire flash du KVM)
config-locked = La configuration par logiciel est désactivée sur le KVM.
config-locked-how = Pour la réactiver : Réglages → Contrôle par logiciel, puis appuyez sur la touche du KVM.

## Clavier

hotkeys = Raccourcis
hotkey-toggle = Changer de PC
hotkey-next = Hôte suivant
hotkey-host = Choisir { $host }
hotkey-prev = Hôte précédent
hotkey-next-profile = Profil suivant
hotkey-prev-profile = Profil précédent
hotkey-profile = Choisir le profil { $profile }
hotkey-aux2-port = Faire défiler le mode du port Aux 2
hotkey-add = Ajouter un raccourci
hotkey-remove = Supprimer
hotkey-action-hover = Ce que fait ce raccourci
hotkeys-full = Les { $max } raccourcis sont tous utilisés.
hotkeys-none = Aucun raccourci pour l'instant.
cant-learn-not-active = Seul le PC actif peut apprendre un raccourci : basculez d'abord le KVM sur ce PC
cant-learn-no-keyboard = Aucun clavier que le KVM lit n'est branché
learning-another = Un autre raccourci est en cours d'apprentissage
learned-with-warning = { $warning } Appliquer l'utilise quand même.
learned = Appris depuis le clavier. Appliquer l'utilise.
not-learned = Non appris : { $why }.
kvm-no-answer = Le KVM n'a pas répondu.
learn-hint-sim = Ou appuyez sur Apprendre, puis sur le raccourci dans cette fenêtre : il joue le clavier du KVM simulé.
learn-hint = Ou appuyez sur Apprendre, puis sur le raccourci au clavier du KVM.
press-the-hotkey = Appuyez… { $seconds } s
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
keymap-note = L'application Vial modifie la partie clavier de la keymap du profil actif (page Keymaps), ou de la keymap 0 tant que le profil n'en utilise aucune, pourvu que son interface soit affichée. Changer l'interface fait redétecter le KVM par les deux PC (environ 1 s).
vial-interface = Interface Vial :
vial-off = Désactivée
vial-auto = Automatique (si aucun appareil en miroir n'a besoin du point de terminaison)
vial-always = Toujours

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
installed-module-next = Installé ; le KVM redémarre, puis le module { -wireless(case: "lower") } reçoit son firmware.
installed-module = Le module { -wireless(case: "lower") } est mis à jour ; il redémarre.
update-complete = Terminé : le KVM a le firmware { $version }.
update-complete-module = Terminé : le KVM et son module { -wireless(case: "lower") } ont le firmware { $version }.
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
boot-kvm-previous = { -product } avec le firmware { $version } et la répartition de la flash d'avant 0.1.51. La préparer écrit la nouvelle répartition et le firmware.
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
mismatch-title = Firmware d'une autre version
mismatch-note = Le firmware du KVM et ce programme sont de versions différentes : tant qu'elles diffèrent, seule la mise à jour du firmware fonctionne. Installez le firmware fourni avec ce programme.
not-allowed-title = Autoriser cet ordinateur
not-allowed-note = Cet ordinateur atteint le KVM par la liaison { -wireless(case: "lower") }, comme l'un de ses hôtes { -wireless(case: "lower") }. Il ne peut piloter le KVM qu'une fois autorisé par la touche du KVM : demandez ici, puis appuyez brièvement sur la touche du KVM. (Un PC relié par USB peut aussi l'autoriser, sur la page { -wireless }.)
not-allowed-ask = Demander au KVM
host-may-control-check = Peut piloter le KVM
host-may-control-hover = Son logiciel { -product } peut modifier les réglages et basculer par la liaison { -wireless(case: "lower") } (pas de mise à jour du firmware : elles passent par USB). L'activer attend un appui sur la touche du KVM.
fw-over-wireless = Les mises à jour du firmware passent par USB : cet ordinateur atteint le KVM par la liaison { -wireless(case: "lower") }. Mettez à jour depuis un PC relié au KVM, ou branchez celui-ci sur un port PC.
looking-for-kvm = Recherche du KVM…
plug-in-kvm = Branchez le KVM sur cet ordinateur.
udev-hint = Installé depuis les sources ? Installez alors software/odkvmctl/udev/70-opendyn-kvm.rules (DEVELOPMENT.md).
try-sim-title = Pas encore de { -product } ?
try-sim-note = Essayez l'application avec un KVM simulé. Le basculement, les ports, les raccourcis, les profils et les mises à jour du firmware agissent sur un KVM fictif ; ce qui n'en a pas besoin est réel : les écrans configurés sur la page Écrans changent d'entrée en même temps (DDC/CI), et la page { -hop } peut détecter les écrans de cet ordinateur. Le simulateur s'arrête à la fermeture de l'application.
try-sim = Essayer avec un KVM simulé
sim-banner-title = KVM simulé
sim-banner = Rien ici n'atteint un vrai KVM. Les écrans configurés sur la page Écrans sont réels et basculent en même temps.
sim-leave = Quitter le simulateur
sim-kvm-found = Un { -product } est branché.
sim-use-kvm = L'utiliser
web-connect-note = Rien ici n'atteint encore un vrai KVM. Connectez le vôtre : par USB depuis un PC qui y est relié, ou par la liaison { -wireless(case: "lower") } depuis un ordinateur appairé comme l'un de ses hôtes. Le navigateur demande quel appareil.
web-connect-usb = Connecter par USB…
web-connect-wireless = Connecter par la liaison { -wireless(case: "lower") }…
web-no-api = Ce navigateur ne peut pas atteindre un KVM (il n'a ni WebHID ni Web Bluetooth) : utilisez un navigateur basé sur Chromium, ou l'application { -product }.
web-connected = Un vrai KVM, atteint depuis ce navigateur.
web-back-to-sim = Revenir au simulateur
web-kvm-gone = Le KVM a disparu : retour au simulateur.
web-describe-usb = { $name } par USB, dans ce navigateur
web-describe-wireless = { $name } par la liaison { -wireless(case: "lower") }, dans ce navigateur

## Hôtes sans fil (la fonction Bluetooth ; son nom est le terme -wireless)

host-connecting = connexion…
host-hover = { -wireless }, { $state }
send = Envoyer
cancel-pairing = Annuler l'association
host-pairing = Un hôte { -wireless(case: "lower") } s'associe
passkey-text = L'hôte affiche six chiffres : tapez-les au clavier du KVM et appuyez sur Entrée (Échap annule), ou saisissez-les ici. D'ici là, le clavier du KVM n'écrit nulle part ailleurs.
passkey-typed = Au clavier du KVM : { $digits }
module-is = Le module est { $state }.
module-needs-update = Le module { -wireless(case: "lower") } doit être mis à jour.
module-needs-update-note = Il a un autre firmware que le KVM, ou aucun. Le KVM ne garde pas le firmware du module : cette application l'envoie, de la version du KVM. D'ici là le { -wireless(case: "lower") } est coupé ; les deux PC fonctionnent comme avant.
update-module = Mettre à jour le module { -wireless(case: "lower") }
module-programming-note = Son firmware vient de cet ordinateur, à travers le KVM, ce qui prend environ 20 s. Les hôtes { -wireless(case: "lower") } se reconnectent ensuite.
module-on-port = Le module est une carte ESP32 sur le { $port }.
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
fault-bt-settings-held = un hôte { -wireless(case: "lower") } change ses réglages trop souvent ; le module ne les enregistre plus, pour ménager sa flash
kvm-refused = Le KVM a refusé : { $why }.

## Écrans qui suivent le KVM (DDC/CI)

profile-n = Profil { $n }
arrangement-n = Disposition { $n }
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
ddc-unsupported-os = Le DDC/CI n'est pris en charge que sous Windows, Linux et macOS
ddc-no-channel = le système n'offre pas de DDC/CI sur cette connexion (essayez un autre port, câble ou adaptateur)
ddc-no-access = { $path } : pas d'accès. Installez le paquet, ou la règle udev du README, puis reconnectez-vous.
ddc-no-i2c-dev = { $path } : absent, le module i2c-dev n'est pas chargé. Installez le paquet, ou chargez-le comme l'indique le README.

## Communication avec le KVM

error-timeout = le KVM n'a pas répondu
error-device = le KVM a refusé : { $why }
error-protocol = erreur de protocole : { $error }
protocol-unexpected = réponse inattendue { $answer }
protocol-update-ended = la mise à jour s'est terminée dans l'état { $state }
error-not-confirmed = la touche du KVM n'a pas été pressée à temps : rien n'a changé
error-mismatch = Ce KVM utilise le firmware { $firmware }, avec lequel ce programme ({ $app }) ne peut pas communiquer : installez le firmware fourni avec lui, sur la page Firmware ou avec odkvmctl update.
host-n = Hôte { $n }
hid-open-linux = { $error } (installez software/odkvmctl/udev/70-opendyn-kvm.rules pour un accès sans root)
no-kvm-found = aucun { -product } trouvé
ble-describe = { $name } par la liaison { -wireless(case: "lower") } ({ $address })
ble-lost = la liaison { -wireless(case: "lower") } avec le KVM a disparu
ble-gone = le service de contrôle du KVM n'est plus sur la liaison { -wireless(case: "lower") }
ble-no-adapter = cet ordinateur n'a pas d'adaptateur { -wireless(case: "lower") } allumé

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
img-layout-changed = La répartition de la flash a changé avec le firmware { $since } : le firmware { $image } ne peut pas remplacer par un port PC le { $kvm } de ce KVM. Installez-le par le port PWR/SERVICE (odkvmctl reboot bootloader, ou le trou maintenu enfoncé au branchement), où préparer la carte écrit la nouvelle répartition.
img-not-first = pas un UF2 de première installation (kvm-first.uf2 : table des partitions et firmware)
fw-read = { $file } : { $error }
fw-not-in-folder = pas de { $name } dans { $folder }
fw-module-mismatch = { $file } est le firmware { $module } du module { -wireless(case: "lower") } : il doit venir de la même version que le firmware du KVM ({ $version }).
fw-module-damaged = { $file } n'est pas un firmware du module { -wireless(case: "lower") } (endommagé ?)
fw-no-module = pas de kvm-module-*.bin (le firmware du module { -wireless(case: "lower") }) dans { $folder }
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
cli-port = Afficher un port en façade, régler son mode ou son routage, ou redétecter son appareil
cli-no-aux1 = Ce KVM n'a pas de port Aux 1 : les cartes l'ont à partir de la 0.19.0.
cli-profile = Lister les profils, en activer un (profile next, profile 1, profile gaming), ou en modifier un (profile 1 name gaming, profile 1 mouse passthrough, profile 1 other passthrough pc2, profile 1 wireless1 - host2)
cli-profile-which = Le numéro ou le nom d'un profil, ou next
cli-profile-setting = name, arrangement, ou un appareil : keyboard, mouse, other, wireless1-wireless4 (puis son mode, - le garde)
cli-hotkey = Les raccourcis : une liste de 16 entrées au plus, chacune une action et son raccourci ; sans sous-commande, la liste
cli-hotkey-list = Afficher la liste
cli-hotkey-add = Ajouter une entrée : une action et son raccourci
cli-hotkey-set = Changer le raccourci d'une entrée, et avec --action ce qu'elle fait
cli-hotkey-remove = Supprimer une entrée
cli-hotkey-action = Ce qu'elle fait : toggle-pc, next-host, prev-host, select-pc1, select-pc2, select-host1 à select-host4, next-profile, prev-profile, select-profile0 à select-profile3, other-port
cli-hotkey-new-action = Ce qu'elle fait désormais (comme pour add)
cli-hotkey-spec = Le raccourci : double:scrolllock, double:rctrl:noswallow, ctrl+alt+1, f13, none ; ou learn, pour que le KVM l'apprenne des prochaines touches pressées à son clavier
cli-hotkey-entry = Le numéro de l'entrée dans la liste (1–16)
cli-hotkeys-none = Aucun raccourci.
cli-hotkeys-full = les { $max } entrées de la liste sont utilisées : supprimez-en une d'abord
cli-hotkey-no-entry = il n'y a pas d'entrée { $n } : la liste a les entrées 1 à { $max }
cli-hotkey-unused = l'entrée { $n } est inutilisée : donnez-lui une action avec --action, ou utilisez add
cli-hotkey-removed = Entrée { $n } supprimée.
cli-config = Lire et modifier les réglages
cli-identify = Faire clignoter les LED du KVM
cli-update = Installer un firmware : sur une carte qui attend dans son chargeur d'amorçage sur le port PWR/SERVICE s'il y en a une, sinon par un port PC celui du RP2354B (demande un appui sur la touche du KVM), puis celui du module { -wireless(case: "lower") }
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

## odkvmctl : sortie

cli-port-auto = auto : { $why }
cli-port-set-to = réglé sur { $mode } : { $why }
cli-status-head = { -product } ({ $device }) : firmware { $firmware }, AT32 { $at1 } / { $at2 }, matériel { $hardware }, numéro de série { $serial }
cli-module-firmware = Module { -wireless(case: "lower") } : firmware { $version }
cli-module-state = Module { -wireless(case: "lower") } : { $state }
cli-module-port = Module { -wireless(case: "lower") } : une carte ESP32 sur le { $port }
cli-module-update-hint = le module { -wireless(case: "lower") } doit être mis à jour : { $command }
cli-waiting-for-kvm = Attente du redémarrage du KVM…
cli-kvm-not-back = le KVM n'est pas revenu avec le nouveau firmware en 90 s
cli-active-host = Actif : hôte { -wireless(case: "lower") } { $slot } « { $host } » (PC de départ { $pc })
cli-active = Actif : { $target }
cli-this-computer = cet ordinateur est { $pc }
cli-profile-line = Profil : { $n } ({ $name })
cli-host-slot = Hôte { $slot }
cli-pairing-open = l'association { -wireless(case: "lower") } est ouverte (encore { $seconds } s)
cli-passkey-waits = un hôte en cours d'association attend son code : tapez-le au clavier du KVM, puis Entrée
cli-supply-sags = 5 V fléchissent
cli-supply-low = 5 V trop bas
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
cli-update-no-module = Pas de module { -wireless(case: "lower") } : rien d'autre à mettre à jour.
cli-module-current = Le module { -wireless(case: "lower") } a déjà ce firmware.
cli-module-up = Module { -wireless(case: "lower") } en marche : firmware { $version }
cli-module-not-up = le module { -wireless(case: "lower") } n'a pas démarré ({ $state })
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
cli-update-module = Seulement le firmware du module { -wireless(case: "lower") } (le kvm-module-*.bin de la version du KVM), quel que soit celui qu'il a
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
hop-state-no-screens = l'ordinateur actif n'a aucun écran dans la disposition de ce profil
hop-state-boot-protocol = désactivé tant que le PC est dans son setup firmware
accel-flat = Linéaire (vitesse seule)
accel-precision = Courbe de précision, comme sous Windows
accel-adaptive = Adaptative, comme sous Linux
mapping-desktop = Sur tous ses écrans (Linux)
mapping-main-screen = Sur son écran principal seulement (Windows)
units-physical = pixels physiques (Windows, X11)
units-logical = pixels logiques (Wayland)
bt-pointer-absolute = Absolu (exact)
bt-pointer-relative = Relatif (pour les systèmes sans pointeur absolu)
parse-screen = '{ $value }' : écrivez un écran sous la forme pc1 2560x1440 150% at 1920,0 desk 0,0 (pc1, pc2, ou host1–host4 pour un hôte { -wireless(case: "lower") } ; mise à l'échelle et positions facultatives ; ajoutez no-hop pour que le pointeur n'y aille pas, fixed pour garder sa place sur son ordinateur), ou off
parse-range = '{ $value }' : un nombre de { $min } à { $max }
screens-none-found = aucun écran trouvé
screens-xrandr-missing = impossible de lancer xrandr pour lister les écrans ({ $error })
screens-xrandr-failed = xrandr n'a pas pu lister les écrans : { $error }
screens-macos-failed = macOS n'a pas pu lister les écrans
screens-unsupported-os = trouver les écrans n'est possible que sous Windows, macOS et Linux
profile-hop = Saut
profile-hop-hover = { -hop } dans ce profil (sa propre page)
hop-what = Le KVM déplace lui-même le pointeur sur les écrans disposés ci-dessous. Poussez-le au-delà d'un bord vers l'écran d'un autre ordinateur (l'autre PC, ou un hôte { -wireless(case: "lower") }), et le KVM bascule sur celui-ci, clavier compris.
hop-turn-on-absolute = Activer le pointeur absolu
hop-mouse-normalized = Passer le port souris en Normalisé dans ce profil
hop-in-profiles = Disposition de chaque profil
hop-profile-active = { $name } (actif)
hop-absolute-pointer = Pointeur absolu sur la souris du KVM
hop-absolute-pointer-note = Le { -hop(case: "lower") } en a besoin. L'activer ou le désactiver fait reconnecter une fois les appareils du KVM sur les deux PC.
hop-arrangement = Disposition
hop-arrangement-note = Faites glisser les écrans à leur place. Le pointeur passe là où deux écrans se touchent et saute l'espace entre eux ; un bord sans rien à côté retient le pointeur.
hop-overlap = Certains écrans se chevauchent : le pointeur ne peut pas passer de l'un à l'autre. Un moniteur relié aux deux PC : excluez-le du { -hop(case: "lower") } sur le PC qui ne l'affiche pas, et posez-le sur l'écran de l'autre PC.
hop-add-screen = Ajouter un écran { $pc }
hop-detect = Détecter les écrans de { $pc }
hop-detect-hover = Les écrans de cet ordinateur, avec leur résolution, leur mise à l'échelle et leur place dans ses réglages d'affichage
hop-remove-screen = Retirer
hop-draft = Pas encore appliqué : le KVM garde sa disposition jusqu'à ce que vous appliquiez.
hop-applying = Appliqué : en attente de la confirmation du KVM. S'il refuse, les modifications restent ici pour réessayer.
hop-apply = Appliquer
hop-discard = Annuler les modifications
hop-mapping-of = { $pc } place le pointeur
hop-units-of = Unités des réglages d'affichage de { $pc }
hop-arrangement-none = aucune (désactivé)
hop-arrangement-in-use = { $name } (utilisée)
hop-arrangement-name = Nom
hop-bt-pointer-of = Pointeur de { $host }
hop-bt-pointer-note = Un hôte { -wireless(case: "lower") } reçoit un pointeur absolu comme les PC : son curseur va exactement là où se trouve le pointeur du KVM. Un système qui ne prend pas de pointeur absolu (certaines tablettes et certains téléphones) a besoin de « Relatif » : le KVM pousse son curseur dans le coin supérieur gauche quand le pointeur arrive sur lui, puis le déplace à partir de là. L'hôte accélère ce mouvement, donc son curseur s'écarte du pointeur du KVM jusqu'à ce qu'il soit poussé contre un bord.
hop-mapping-note = Windows place un pointeur absolu sur son écran principal seulement. Le KVM atteint ses autres écrans en y déplaçant le curseur comme une souris, et Windows accélère ce mouvement : désactivez « Améliorer la précision du pointeur » sur un tel PC pour des bords exacts, ou laissez { -product } tourner dessus, qui y place alors le curseur lui-même.
hop-mapping-windows = Cet ordinateur tourne sous Windows, qui place le pointeur absolu sur son écran principal seulement : sur tous les écrans, le pointeur arrive au mauvais endroit et les autres écrans restent hors d'atteinte. Choisissez « Sur son écran principal seulement (Windows) ».
hop-place-cursor-windows = Placer le curseur depuis cette application sur les autres écrans de cet ordinateur
hop-place-cursor-macos = Placer le curseur depuis cette application sur tous les écrans de cet ordinateur
hop-place-cursor-hover = Facultatif : le curseur va exactement là où est le pointeur du KVM, quoi que l'accélération du système fasse du mouvement. Sans cette application, ou si cette option est désactivée, le KVM utilise son pointeur absolu et, là où il n'arrive pas, un mouvement relatif.
hop-place-placing = Le curseur est placé.
hop-place-no-kvm = Pas de placement : aucun KVM ne répond.
hop-place-mismatch = Pas de placement : le firmware du KVM est d'une autre version que ce programme.
hop-place-not-allowed = Pas encore de placement : macOS laisse cette application déplacer le curseur une fois qu'elle est autorisée dans Réglages Système, Confidentialité et sécurité, Accessibilité.
hop-place-allow = Autoriser…
hop-place-other-desktop = Pas de placement pour l'instant : l'écran de connexion, l'écran de verrouillage ou une demande d'administrateur a la saisie. Le KVM déplace le pointeur à sa façon en attendant.
hop-place-refused = Pas de placement pour l'instant : Windows a refusé (une fenêtre d'administrateur au premier plan ?). Le KVM déplace le pointeur à sa façon en attendant et redemande bientôt.
hop-no-screens = Pas encore d'écran : ajoutez-en un pour chaque ordinateur.
hop-main-screen = principal
hop-screen-hover = Cliquez pour modifier (cliquez encore pour un écran en dessous), faites glisser pour déplacer
hop-screen-pc = Ordinateur
hop-screen-resolution = Résolution
hop-screen-scale = Mise à l'échelle
hop-screen-desk = Position sur son ordinateur
hop-screen-at = Position ici
hop-screen-used = Le { -hop(case: "lower") } y va
hop-screen-used-note = Désactivé pour un moniteur relié aux deux PC qui affiche l'autre : posez-le sur l'écran de ce PC. Son PC le compte toujours, si bien que le pointeur arrive au bon endroit sur ses autres écrans.
hop-screen-off-short = exclu du { -hop(case: "lower") }
hop-screen-desk-note = L'emplacement de l'écran dans les réglages d'affichage de son ordinateur, en pixels de cet ordinateur (logiques sur un bureau Wayland). Sous Windows, l'écran principal est en 0, 0.
hop-detected = { $count ->
    [one] { $pc } a maintenant 1 écran, comme cet ordinateur.
   *[other] { $pc } a maintenant { $count } écrans, comme cet ordinateur.
} { $pc } place le pointeur : { $mapping }.
hop-pointer = Pointeur
hop-accel = Accélération
hop-speed = Vitesse
hop-resistance = Résistance du bord
hop-corner = Coins
hop-pointer-note = Les ordinateurs n'accélèrent pas un pointeur absolu, alors le KVM s'en charge. La résistance du bord est la distance dont il faut pousser le pointeur contre un bord avant qu'il passe à un autre ordinateur ; les coins sont la distance depuis chaque bout de ce bord où il ne passe jamais, pour que les coins restent faciles à atteindre. Pendant un glisser, il ne passe jamais.
cli-screens = Saut d'écran : les dispositions des écrans des ordinateurs et le mouvement du pointeur
cli-screens-list = Afficher les dispositions et les réglages du pointeur
cli-screens-detect = Mettre les écrans de cet ordinateur dans la disposition
cli-screens-which-pc = le KVM n'indique pas quel PC est cet ordinateur (simulateur ou port de service) : réglez plutôt les écrans avec odkvmctl config set arrangement0-screen0 …
cli-hop-mapping = { $pc } place le pointeur : { $mapping }
cli-hop-units = Les réglages d'affichage de { $pc } comptent en { $units }
cli-hop-pointer = Pointeur : { $accel }, vitesse { $speed }, résistance du bord { $resistance } px, coins { $corner } px
cli-screens-arrangement = La disposition, par numéro (0–3) ou par nom ; par défaut celle du profil actif
cli-no-arrangement = pas de disposition '{ $which }' : un nombre de 0 à 3 ou un nom
cli-arrangement-unused = dans aucun profil
cli-arrangement-used = dans { $profiles }
cli-hop-bt-pointer = Pointeur de { $host } : { $pointer }
cli-profile-no-hop = pas de { -hop(case: "lower") }
cli-profile-hop = { -hop(case: "lower") } : { $name }
hop-out-of-date = Les réglages d'affichage de cet ordinateur diffèrent de la copie de ses écrans dans le KVM, si bien que le pointeur arrive au mauvais endroit ici : détectez à nouveau ses écrans.
cli-hop-out-of-date = { hop-out-of-date } (odkvmctl screens detect)

## Wireless devices (keyboards, mice paired with the module) and where each device's input goes

mode-off = Désactivé (ses entrées ne vont nulle part)
route-follow = Suit l'hôte actif
route-pin = Fixé sur { $host }
device-wireless-n = Appareil { -wireless(case: "lower") } { $n }
device-kind-keyboard = clavier
device-kind-mouse = souris
device-kind-gamepad = manette
device-kind-pen = stylet
device-kind-touchpad = pavé tactile
device-kind-presenter = télécommande de présentation
device-kind-other = périphérique d'entrée
device-pairing-connecting = Connexion…
device-pairing-pairing = Association…
device-pairing-reading = Lecture de ce qu'il envoie…
device-pairing-paired = Associé
device-pairing-failed = Échec de l'association : mettez l'appareil en mode d'association près du KVM et réessayez
parse-profile-routes = '{ $value }' : huit routages dans le même ordre, chacun follow, pc1, pc2 ou host1–host4
cli-device = Appareils { -wireless(case: "lower") } (claviers, souris) des KVM avec le module : les lister (par défaut), en chercher un et l'associer, en oublier un, régler son mode ou son routage
cli-keymap = Keymaps : les afficher, définir ce que fait une commande ou un axe, apprendre des commandes, donner une keymap à un profil
cli-keymap-show = Ce qu'une keymap (0–3 ; toutes sans indication) fait autrement qu'une keymap neuve, et quels profils l'utilisent
cli-keymap-set = Définir ce que fait une commande sur une couche d'une keymap : <keymap 0–3> <couche 0–7> <commande> <keycode>
cli-keymap-axis = Définir ce que fait un axe sur une couche d'une keymap
cli-keymap-axis-input = mouse-x, mouse-y, wheel, pan, x, y, z, rx, ry, rz, slider, dial, brake, accelerator
cli-keymap-axis-target = trans, none, mouse-x, mouse-y, wheel, pan, buttons, pad-x … pad-dial
cli-keymap-axis-deadzone = Zone morte, en pour cent de la déviation complète
cli-keymap-axis-param = Stick vers souris : pas par 8 ms à pleine déviation ; souris vers stick : vitesse du retour (256es par 8 ms, 0 : aucun) ; vers boutons : le seuil (256es)
cli-keymap-learn = Afficher les commandes des appareils normalisés pendant que vous les utilisez, quelques secondes
cli-keymap-use = Donner à un profil (0–7) une keymap (0–3) ou aucune
cli-keymap-range = les keymaps sont 0–3, les couches 0–7
cli-keymap-title = Keymap { $keymap } (utilisée par : { $profiles })
cli-keymap-unused = aucun profil
cli-keymap-entry = couche { $layer } : { $control } → { $keycode }
cli-keymap-axis-entry = couche { $layer } : { $axis } → { $target } × { $scale }
cli-keymap-fresh = comme neuve : chaque commande fait ce qui est écrit
cli-keymap-learning = Utilisez les commandes dont vous voulez connaître le nom ({ $seconds } s) …
cli-keymap-used = Le profil { $profile } utilise la keymap { $keymap }.
cli-control-down = enfoncée
cli-control-up = relâchée
cli-ev-control = commande { $control } { $state }
cli-ev-axis = axe { $axis } : { $value }
cli-calibrate = Les axes d'une manette : afficher leur calibrage, le mesurer, le définir ou le réinitialiser
cli-calibrate-device = keyboard, mouse, other, wireless1 … wireless4
cli-calibrate-show = Minimum, centre, maximum et zone morte de chaque axe
cli-calibrate-set = Définir un axe : minimum, centre, maximum (−32767 … 32767) et zone morte (256es)
cli-calibrate-measure = Mesurer : amener chaque stick et gâchette en butée, puis relâcher
cli-calibrate-reset = Revenir à non calibré
cli-calibrate-not-pad = seuls les axes d'une manette se calibrent (x … accelerator)
cli-calibrate-move = Amenez chaque stick et gâchette en butée ({ $seconds } s) …
cli-calibrate-let-go = Relâchez tout …
cli-calibration = { $axis } : { $min } … { $centre } … { $max }, zone morte { $deadzone }
cli-monitor = Les rapports bruts d'un appareil à mesure qu'ils changent, et les champs déclarés par son descripteur : pour trouver les bits qu'une surcharge de champ doit lire
cli-monitor-iface = Interface { $iface } : { $fields } champs
cli-monitor-field = rapport { $id }, bit { $bit } : { $count } × { $bits } bits, usage { $usage }
cli-monitor-start = Utilisez l'appareil ({ $seconds } s) ; les octets changés sont marqués d'un * …
cli-monitor-report = { $iface } : { $bytes }
cli-field = Les surcharges de champs d'un appareil : des bits de ses rapports lus comme boutons ou axes de manette
cli-field-list = Les surcharges de champs de l'appareil
cli-field-set = Faire de bits une commande : <n 0–7> <interface> <ID de rapport (0 : aucun)> <premier bit après l'ID de rapport> <bits 1–16> <cible>
cli-field-target = pad1–pad32, ou un axe : x, y, z, rx, ry, rz, slider, dial, brake, accelerator
cli-field-remove = Supprimer une surcharge de champ
cli-field-entry = { $n } : interface { $iface }, rapport { $id }, { $bits } bits à partir du bit { $bit } → { $target } { $flags }
cli-field-target-bad = '{ $value }' : pas une cible (pad1–pad32, ou un axe : x, y, z, rx, ry, rz, slider, dial, brake, accelerator)
cli-outputs = Ce que chaque hôte se voit offrir et a pris : clavier, souris, touches multimédia, pointeur absolu, manette
cli-outputs-none = { $host } : rien (non configuré)
cli-outputs-all = { $host } : { $outputs }
cli-outputs-idle = { $host } : ne prend rien pour l'instant (offert : { $outputs })
cli-outputs-some = { $host } : { $taken } ; non pris : { $left }
cli-layout = La disposition du clavier de chaque hôte, selon laquelle le texte des macros est tapé (et que montre l'image du clavier de l'appli)
cli-layout-host = pc1, pc2, host1 … host4
cli-layout-name = us, gb, de, ch, fr, es, it, pt, no, se, dk, jp, br
cli-layout-entry = { $host } : { $layout }
cli-device-map-which = keyboard, mouse, other, wireless1 … wireless4, ou l'emplacement 1–4 d'un appareil { -wireless(case: "lower") }
cli-device-map-iface = Interface { $iface } :
cli-keymap-fallback = Donner à un profil (0–7) une keymap (0–3) pour un hôte qui ne prend pas de manette, ou aucune
cli-keymap-fallback-used = Là où la manette ne va pas, le profil { $profile } utilise la keymap { $keymap }.
cli-keymap-macro = Les macros : les afficher, ou en définir une : <n 0–15> <étape> … (none : vide)
cli-keymap-macro-steps = text:bonjour, tap:ctrl+c, down:shift, up:shift, delay:100 (keycodes comme pour keymap set)
cli-keymap-dance = Les tap dances : les afficher, ou en définir une : <n 0–31> <tape> [<maintien> [<double tape> [<tape et maintien>]]]
cli-keymap-dance-term = Son délai de tape en ms (0 : celui du réglage)
cli-keymap-combo = Les combinaisons : les afficher, ou en définir une : <n 0–31> <keycodes> <sortie>
cli-keymap-combo-keys = Jusqu'à quatre keycodes séparés par des virgules (pad1,pad2), ou none
cli-combo-keys = Une combinaison a un à quatre keycodes, séparés par des virgules
cli-keymap-override = Les substitutions de touches : les afficher, ou en définir une : <n 0–31> <déclencheur> <remplacement>
cli-keymap-override-mods = Modificateurs qui doivent être maintenus (ctrl+shift)
cli-keymap-override-negative = Modificateurs qui ne doivent pas être maintenus
cli-keymap-override-suppress = Modificateurs retenus pendant le remplacement
cli-keymap-override-layers = Les couches où elle agit, séparées par des virgules (par défaut : toutes)
cli-keymap-off = L'enregistrer désactivée
cli-keymap-repeat = Les touches Alternate Repeat : les afficher, ou en définir une : <n 0–31> <keycode> <alternative>
cli-keymap-repeat-mods = Modificateurs qu'elle permet
cli-keymap-setting = Les réglages QMK des keymaps : les afficher, ou en définir un : <nom> <valeur>
cli-macro-entry = macro { $n } : { $steps }
cli-macro-empty = macro { $n } : vide
cli-macro-range = les macros sont 0–{ $max }
cli-macro-too-long = les macros ne tiennent pas dans leurs 2 Ko (ou une étape de texte contient un caractère hors ASCII)
cli-dance-entry = tap dance { $n } : tape { $tap }, maintien { $hold }, double tape { $double }, tape et maintien { $taphold }, délai { $term } ms
cli-combo-entry = combinaison { $n } : { $keys } → { $output }
cli-override-entry = substitution { $n } : { $mods }+{ $trigger } → { $replacement } ({ $state })
cli-repeat-entry = alternate repeat { $n } : { $keycode } → { $alt } ({ $state })
cli-entry-on = actif
cli-entry-off = désactivé
cli-setting-entry = { $name } = { $value }
cli-device-list = Les quatre emplacements d'appareil : état, nom, adresse ; pendant la recherche, les appareils trouvés
cli-device-scan = Chercher des appareils à associer pendant 60 s et les lister au fur et à mesure (mettez l'appareil en mode d'association)
cli-device-scan-stop = stop arrête la recherche plus tôt
cli-device-pair = Associer un appareil trouvé : son numéro dans la liste, ou son adresse
cli-device-forget = Oublier l'association d'un appareil : emplacement 1-4, ou all
cli-device-log = Afficher le journal récent du module sans fil tel que le KVM le garde : connexions, appairage, pourquoi un appareil a été lâché
cli-device-log-empty = Le KVM ne détient encore aucune ligne du journal du module.
cli-device-map = Afficher la table de rapports que le KVM détient pour l'emplacement 1-4, et ce que son analyseur en lit
cli-device-map-none = Le KVM ne détient aucune table de rapports pour cet emplacement (aucun appareil appairé, ou sa table pas encore lue).
cli-device-map-bytes = Table de rapports, { $bytes } octets :
cli-device-map-ok = Le KVM la lit : { $fields } champs.
cli-device-map-incomplete = Le KVM en lit une partie : { $fields } champs ; le reste dépasse ce qu'il peut contenir.
cli-device-map-error = Le KVM ne peut pas la lire : { $error }.
cli-route = follow (l'hôte actif), ou fixé sur un : pc1, pc2, host1-host4
cli-profile-route = Pour un appareil, aussi son routage : follow, pc1, pc2, host1-host4
cli-ev-devices-changed = Appareils { -wireless(case: "lower") } : recherche ou association modifiée
cli-ev-device-changed = appareil { -wireless(case: "lower") } { $slot } modifié
cli-pinned = fixé
cli-device-slot = Appareil { $slot }
cli-device-ready = connecté
cli-battery = batterie { $percent } %
cli-scan-open = Recherche d'appareils : encore { $seconds } s, { $found } trouvés
cli-scan-usage = '{ $value }' : utilisez `device scan` ou `device scan stop`
cli-scan-started = Recherche d'appareils { -wireless(case: "lower") } pendant { $seconds } s : mettez l'appareil en mode d'association près du KVM. Ctrl-C laisse le KVM chercher.
cli-scan-ended = Recherche terminée. Associez-en un avec `odkvmctl device pair <numéro>`.
cli-device-not-found = aucun appareil « { $which } » trouvé : `odkvmctl device scan` les liste
device-passkey = Tapez { $passkey } sur le clavier, puis appuyez sur Entrée.
make-active = Activer
device = Appareil
routing = Ses entrées vont à
port-module-short = le module { -wireless(case: "lower") }
pair-new-device = Associer un appareil
pair-device-note = Cherche des claviers et souris pendant une minute. Mettez l'appareil en mode d'association près du KVM ; il apparaît ci-dessous. Un clavier qui le demande reçoit six chiffres à taper dessus, puis Entrée.
look-for-devices = Chercher des appareils
looking = Recherche · { $seconds } s
found-none = Rien trouvé pour l'instant
device-slots-full = Oubliez d'abord un appareil : les quatre emplacements sont pris
devices = Appareils
devices-note = Les appareils associés sont des appareils du KVM comme ceux des ports en façade : leur mode et où vont leurs entrées se règlent dans la page Ports, dans chaque profil. Un appareil fixé tape dans son hôte quel que soit l'actif.
device-away = absent
device-unsupported = non pris en charge
battery = batterie { $percent } %
forget-device-hover = Supprime l'association. Pour l'associer à nouveau, mettez l'appareil en mode d'association.
ev-device-paired = Associé : { $device } (appareil { -wireless(case: "lower") } { $slot })
ev-device-forgot = Appareil { -wireless(case: "lower") } { $device } oublié

## Erasing every wireless pairing
bt-erase-title = Effacer tous les appairages
bt-erase-note = Oublie tous les hôtes et appareils que le module sans fil a appairés et toutes les clés qu'il détient, puis le redémarre : quand l'appairage échoue sans cesse, ou qu'un hôte s'appaire et décroche aussitôt. Retirez aussi le KVM sur les hôtes et appareils, puis appairez-les à nouveau.
bt-erase-button = Effacer tous les appairages…
bt-erase-really = Tous les hôtes et appareils devront être appairés à nouveau.
bt-erase-confirm = Les effacer
cli-wireless-erase = Oublier tous les hôtes et appareils que le module sans fil a appairés et toutes les clés qu'il détient, puis le redémarrer (quand l'appairage échoue sans cesse)
cli-wireless-erased = Le module oublie tous les appairages et redémarre. Retirez aussi le KVM sur les hôtes et appareils, puis appairez-les à nouveau.
cli-hosts-allow = Laisser un hôte { -wireless(case: "lower") } piloter le KVM par le module, confirmé avec la touche du KVM : depuis un PC, indiquez l'emplacement de l'hôte ; sur l'hôte lui-même, aucun
cli-hosts-allow-slot = l'emplacement de l'hôte (1–4) ; aucun : cet ordinateur, qui atteint le KVM par la liaison { -wireless(case: "lower") }
cli-hosts-disallow = L'hôte { -wireless(case: "lower") } de cet emplacement ne peut plus piloter le KVM
cli-allow-needs-slot = cet ordinateur atteint le KVM par USB : indiquez l'emplacement de l'hôte { -wireless(case: "lower") } (1–4)
cli-allowed-here = Cet ordinateur peut maintenant piloter le KVM par la liaison { -wireless(case: "lower") }.
cli-host-allowed = L'hôte { -wireless(case: "lower") } { $slot } peut maintenant piloter le KVM.
host-may-control = peut piloter le KVM
host-controls-now = son logiciel est connecté
cli-this-computer-host = cet ordinateur
cli-list-wireless = { -wireless }
module-adopt-note = Cette carte ESP32 exécute un autre firmware, le KVM la laisse donc tranquille. Elle peut devenir le module { -wireless(case: "lower") } à la place.
module-adopt = En faire le module { -wireless(case: "lower") }…
module-adopt-really = Son firmware sera remplacé par celui du module { -wireless(case: "lower") }.
module-adopt-confirm = Remplacer son firmware
cli-wireless-adopt = Faire de la carte ESP32 sur un port avant le module { -wireless(case: "lower") }, en remplaçant son firmware (une carte que le KVM laisse tranquille parce qu'elle exécute un autre firmware)
cli-wireless-adopt-port = Le port avant de la carte : keyboard, mouse ou other
cli-wireless-adopt-firmware = Le dossier de version d'où prendre le firmware du module (par défaut : celui livré avec odkvmctl)
cli-adopting = La carte sur le port { $port } devient le module { -wireless(case: "lower") } : attente de la carte dans sa ROM…
cli-adopt-busy = ce KVM a déjà un module { -wireless(case: "lower") }
cli-adopt-empty = rien sur le port { $port }
cli-adopt-not-taken = la carte sur le port { $port } n'est pas devenue le module { -wireless(case: "lower") } ({ $state })
