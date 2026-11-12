# OpenDynKVM: odkvmctl und opendyn-kvm, Deutsch. Fehlt hier eine Meldung, erscheint die englische.

## Marken und Namen. Produkt- und Funktionsnamen sind Terme: hier ändern, und alle Meldungen folgen.
## -wireless ist die Bluetooth-Funktion. Solange das Produkt nicht bei der Bluetooth SIG gelistet ist, heißt sie
## "Funk"; danach beide Formen auf "Bluetooth" setzen. [prefix] steht vor einem Bindestrich ("Funk-Modul").

-product = OpenDynKVM
-wireless = { $case ->
   *[title] Funk
    [prefix] Funk
}
## Wo ein Telefon oder PC eine Tastatur koppelt: das Menü des Geräts, es behält seinen eigenen Namen.
-host-pairing-settings = Bluetooth-Einstellungen
-hop = Bildschirmsprung

product-name = { -product }

## Häufige Wörter

yes = ja
no = nein
on = ein
off = aus

## Fehler, die der KVM meldet

err-unknown = unbekannte Anfrage
err-malformed = fehlerhafte Anfrage
err-locked = am KVM ausgeschaltet (wieder einschalten mit `odkvmctl config set software-config on` und einem Druck auf die Taste am KVM, oder die Taste beim Einschalten des KVM gedrückt halten)
err-busy = beschäftigt, bitte erneut versuchen
err-unsupported = von dieser Firmware nicht unterstützt
err-invalid-value = ungültiger Wert
err-needs-key-press = zum Bestätigen die Taste am KVM drücken
err-no-device = kein Gerät an diesem Anschluss
err-not-active = das kann nur der aktive PC: zuerst den KVM auf diesen PC schalten

## Ein Tastenkürzel lernen

learn-learned = gelernt
learn-timed-out = keine Tasten für ein Kürzel rechtzeitig gedrückt
learn-cancelled = abgebrochen (der KVM hat den PC gewechselt, oder das Lernen wurde abgebrochen)
hotkey-warn-twin = Das ist auch das Kürzel „{ $name }“: ein Druck würde beides auslösen.
hotkey-warn-alone = { $key } allein würde umschalten, und die PCs bekämen die Taste nicht mehr.

## Steuerung durch Software ausschalten

lock-warn-switching = Keiner der PCs kann den KVM dann per Software umschalten: weder mit odkvmctl noch mit der App oder ihrem Tray-Symbol. Die Taste am KVM und die Tastenkürzel funktionieren weiter.
lock-warn-config = Keiner der PCs kann dann per Software Einstellungen ändern: keine Anschlussmodi, Profile, Tastenkürzel, Firmware-Updates oder Neustarts, und Vial wird schreibgeschützt. Der Status bleibt sichtbar.
lock-warn-back-on = Wieder einschalten geht nur mit der Taste am KVM: ein Druck innerhalb von { $seconds } s nach der Anfrage, oder die Taste beim Einschalten des KVM gedrückt halten.

## Verbindung zu einem PC

link-unpowered = aus
link-detached = nicht verbunden
link-enumerating = verbindet
link-connected = verbunden
link-suspended = im Ruhezustand
link-down = Verbindung unterbrochen

## Anschlüsse vorne

port-state-empty = leer
port-state-probing = erkennt
port-state-hosted = aktiv
port-state-passthrough = Durchreichen
port-state-unpowered = ohne Strom
port-state-fault = Fehler
port-state-unsupported = nicht unterstützt

mode-auto = Automatisch
mode-normalized = Normalisiert
mode-mirrored = Gespiegelt
mode-mirrored-identity = Gespiegelt + Identität
mode-passthrough = Durchreichen (folgt dem aktiven PC)
mode-passthrough-pc1 = Durchreichen, fest an PC1
mode-passthrough-pc2 = Durchreichen, fest an PC2
mode-short-auto = Auto
mode-short-normalized = Normalisiert
mode-short-mirrored = Gespiegelt
mode-short-identity = Identität
mode-short-passthrough = Durchreichen
mode-short-pinned-pc1 = Fest PC1
mode-short-pinned-pc2 = Fest PC2

reason-not-auto = von dir eingestellt
reason-fits-normalized = alle Eingaben passen in die normalisierten Schnittstellen
reason-extra-inputs = hat Eingaben, die die normalisierten Schnittstellen nicht tragen
reason-other-port-hid = HID-Gerät am Anschluss „Weitere“
reason-non-hid = hat Schnittstellen, die kein HID sind
reason-xinput = XInput-Gamepad
reason-endpoint-budget = nicht mehr genug Endpunkte, um es zu spiegeln
reason-identity-taken = ein anderer Anschluss nutzt die Identität schon

speed-low = Low-Speed
speed-full = Full-Speed
speed-high = High-Speed

class-keyboard = Tastatur
class-mouse = Maus
class-consumer = Medientasten
class-gamepad = Gamepad
class-xinput = XInput
class-vendor-hid = Herstellereingaben
class-vendor-channel = Herstellerbefehle
class-other-hid = anderes HID
class-audio = Audio
class-video = Video
class-storage = Speicher
class-hub = Hub

## Die Taste am KVM

key-action-none = Nichts
key-action-toggle-pc = Zum anderen PC wechseln
key-action-next-profile = Nächstes Profil
key-action-other-port = Modus des Anschlusses „Weitere“ durchschalten
key-action-next-target = Nächstes Ziel (PCs, dann { -wireless(case: "prefix") }-Hosts)

## Stromversorgung

adv-none = keine
adv-default = USB-Standard
adv-1a5 = 1,5 A
adv-3a = 3 A
power-source-pwr = PWR-Anschluss
power-source-pc = PC-USB

## Firmware-Update

fw-state-idle = bereit
fw-state-receiving = lädt hoch
fw-state-awaiting-key = zum Installieren die Taste am KVM drücken
fw-state-writing = installiert
fw-state-done = fertig
fw-state-failed = fehlgeschlagen

## Das Funkmodul

bt-module-up = läuft
bt-module-starting = startet
bt-module-programming = wird aktualisiert
bt-module-failed = läuft nicht mit seiner Firmware: { -wireless(case: "prefix") } aus bis zum nächsten Einschalten
bt-module-gave-up = nach 5 Versuchen aufgegeben: { -wireless(case: "prefix") } aus
bt-module-absent = nicht bestückt

## Selbsttest

selftest-hub0 = Hub-0 (Anschlüsse vorne) nicht gefunden
selftest-link1 = keine Verbindung zum PC1-Chip
selftest-link2 = keine Verbindung zum PC2-Chip
selftest-hub1 = PC1-Hub funktioniert nicht
selftest-hub2 = PC2-Hub funktioniert nicht
selftest-port-kb = Stromschalter des Tastaturanschlusses
selftest-port-ms = Stromschalter des Mausanschlusses
selftest-port-ot = Stromschalter des Anschlusses „Weitere“
selftest-power = Messwerte am PWR-Anschluss unplausibel
selftest-at32-fw1 = Programmieren des PC1-Chips fehlgeschlagen (nächster Versuch beim nächsten Einschalten)
selftest-at32-fw2 = Programmieren des PC2-Chips fehlgeschlagen (nächster Versuch beim nächsten Einschalten)
selftest-at32-gave-up1 = Programmieren des PC1-Chips nach 5 Versuchen aufgegeben
selftest-at32-gave-up2 = Programmieren des PC2-Chips nach 5 Versuchen aufgegeben
selftest-at32-xtal1 = der Quarz des PC1-Chips schwingt nicht (kein USB zu PC1)
selftest-at32-xtal2 = der Quarz des PC2-Chips schwingt nicht (kein USB zu PC2)
selftest-bt-fw = das { -wireless(case: "prefix") }-Modul läuft nicht mit seiner Firmware (nächster Versuch beim nächsten Einschalten)
selftest-bt-gave-up = Programmieren des { -wireless(case: "prefix") }-Moduls nach 5 Versuchen aufgegeben
selftest-running = läuft
selftest-passed = bestanden
selftest-running-so-far = läuft, bisher: { $failed }
selftest-failed = FEHLGESCHLAGEN: { $failed }

## Warum der KVM umgeschaltet hat

switch-source-key = Taste am KVM
switch-source-hotkey = Tastenkürzel
switch-source-software = Software
switch-source-boot = Start
switch-source-power = der aktive PC wurde ausgeschaltet

## Diagnose

diag-group-system = System
diag-group-clocks = Takte
diag-group-usb = USB
diag-group-power = Stromversorgung
diag-group-pc-chips = PC-seitige Chips
diag-group-wireless = { -wireless }
diag-group-other = Weitere

diag-uptime = Laufzeit
diag-reset-cause = Letzter Neustart
diag-rp-temp = Temperatur RP2354B
diag-selftest = Selbsttest
diag-dropped = Verworfene Engine-Ausgaben
diag-fw-updates = Firmware-Updates über USB
diag-xosc = Quarz (XOSC)
diag-pll-sys = System-PLL
diag-pll-usb = USB-PLL
diag-rosc = Ringoszillator
diag-at-clock = Takt AT32F405 #{ $n }
diag-hub0-speed = Hub-0-Verbindung
diag-hub0-up = Hub-0 erkannt
diag-phy-swap = D+/D−-Tausch (Errata E1)
diag-kb-speed = Gerät am Tastaturanschluss
diag-ms-speed = Gerät am Mausanschluss
diag-ot-speed = Gerät am Anschluss „Weitere“
diag-pc-speed = USB-Geschwindigkeit { $pc }
diag-pc-link = Verbindung { $pc }
diag-pc-resets = Bus-Resets { $pc }
diag-link-errors = SPI-Fehler AT32F405 #{ $n }
diag-hub0-errors = Nicht behandelte Hub-0-Änderungen
diag-enum-fails = Fehlgeschlagene Enumerationen an den vorderen Anschlüssen
diag-vbus = VBUS { $port }
diag-cc = CC { $port }
diag-at-3v3 = 3,3 V am AT32F405 #{ $n }
diag-budget = Strombudget vorne
diag-kb-faults = Fehler Tastaturanschluss
diag-ms-faults = Fehler Mausanschluss
diag-ot-faults = Fehler Anschluss „Weitere“
diag-at-temp = Temperatur AT32F405 #{ $n }
diag-at-fw = Firmware AT32F405 #{ $n }
diag-bt-module = ESP32-C3-Modul
diag-bt-fw = ESP32-C3-Firmware
diag-bt-link-errors = ESP32-C3-UART-Fehler
diag-bt-hosts = Verbundene { -wireless(case: "prefix") }-Hosts
diag-unknown = Eintrag { $id }

reset-software = Neustart per Software
reset-power-on = Einschalten
reset-brown-out = Unterspannung (die Versorgung ist eingebrochen)
reset-glitch = Störung der Versorgung
reset-run-pin = RUN-Pin
reset-watchdog = Watchdog (auch nach einem Update)
reset-debugger = Debugger
reset-other = Ursache { $cause }
hz-stopped = 0 Hz (steht)
clock-crystal = 12-MHz-Quarz
clock-internal = interner Oszillator: kein USB
swap-found = { $state }, zur Laufzeit erkannt
fw-updates-on = ein, Firmware in Partition { $partition }
fw-updates-no-table = aus: keine Partitionstabelle (mit einem Debugger geflasht)
fw-updates-read-failed = aus: das Boot-ROM lieferte die Partitionstabelle nicht ({ $detail })
fw-updates-other-table = aus: eine andere Partitionstabelle ({ $detail })
fw-updates-other-layout = aus: die Partitionen weichen von denen dieser Firmware ab ({ $detail })
fw-updates-unknown-partition = aus: unbekannt, welche Partition läuft ({ $detail })
state-other = Zustand { $state }

diag-help-reset-cause = Orange nach Unterspannung oder einer Störung der Versorgung: die 5 V sind eingebrochen. Gewollte Neustarts haben keine Farbe.
diag-help-rp-temp = Orange ab 70 °C, rot ab 85 °C.
diag-help-selftest = Rot, wenn ein Teil des KVM nicht funktioniert; orange für den Stromschalter eines Anschlusses vorne.
diag-help-dropped = Arbeit, für die die Firmware keinen Platz hatte. Orange, wenn nicht 0.
diag-help-fw-updates = Ob der KVM Firmware-Updates über einen PC-Anschluss annimmt. Orange, wenn nicht: Der Code in Klammern sagt, was seine Prüfung gelesen hat, nützlich für eine Fehlermeldung. Keine Farbe bei einer Platine, die mit einem Debugger geflasht wurde.
diag-help-xosc = Der 12-MHz-Quarz; rot, wenn er nicht schwingt.
diag-help-clk-sys = Gegen den Quarz gemessen. Rot bei 1 % Abweichung, oder wenn nicht 10 % über clk_usb (RP2350-E12).
diag-help-clk-usb = Gegen den Quarz gemessen. Rot bei mehr als 0,25 % Abweichung von 48 MHz (USB Full-Speed).
diag-help-clk-adc = Rot bei mehr als 1 % Abweichung von 48 MHz.
diag-help-at-clock = Rot, wenn der Chip ohne seinen Quarz läuft: dann hat er kein USB zu seinem PC.
diag-help-hub0-speed = Der Hub der Anschlüsse vorne. Rot bei Low-Speed: D+ und D− sind für die Einstellung der PHY vertauscht (Errata E1).
diag-help-phy-swap = Ob das USB des RP2354B D+ und D− tauscht (Errata E1): an bei den Platinen 0.8 bis 0.12.
diag-help-pc-speed = Orange bei Full-Speed: 125-µs-Polling braucht High-Speed. Ein anderes Kabel oder einen anderen Anschluss versuchen.
diag-help-pc-link = Rot, wenn der PC-seitige Chip nicht antwortet; orange, solange der PC ihn nicht eingerichtet hat.
diag-help-link-errors = Fehlgeschlagene Austausche mit dem PC-seitigen Chip seit dem Start, sobald er zum ersten Mal geantwortet hat.
diag-help-hub0-errors = Portänderungen, die Hub-0 meldet und der USB-Host des KVM nicht behandelt; sie werden gelöscht, damit sie die anderen Ports nicht verdecken. Höchstens ein paar; mehr ist eine Fehlermeldung wert.
diag-help-enum-fails = Geräte an den vorderen Anschlüssen, die der KVM nicht lesen konnte, jedes einige Male neu versucht, indem der Anschluss aus- und eingeschaltet wird. Ein Gerät, das beim Lesen abgezogen wird, zählt auch.
diag-help-vbus = Gut 4,75–5,50 V; orange 4,40–4,75 V (zu niedrig) oder bis 5,70 V; darüber rot. Keine Farbe unter 0,8 V: nichts speist ihn.
diag-help-cc = Rot über 2,3 V (5 V auf CC: Kabel oder Kontakt). Orange unter 0,2 V bei anliegenden 5 V: ein Kabel ohne Rp.
diag-help-3v3 = Gut 3,135–3,465 V (±5 %), gegen eine Referenz mit ±3,3 %; orange bis 3,0 / 3,6 V.
diag-help-budget = Orange, wenn die angeschlossenen Geräte mehr ziehen könnten, als die Quelle bietet: den PWR-Anschluss nutzen.
diag-help-faults = Überstromabschaltungen des Stromschalters dieses Anschlusses.
diag-help-at-temp = Orange ab 85 °C, rot ab 105 °C. Von Chip zu Chip bis zu 20 °C daneben.
diag-help-at-fw = Orange, wenn sie von der des RP2354B abweicht: wird neu programmiert, oder das Programmieren ist fehlgeschlagen.
diag-help-bt-module = Orange, solange der KVM es programmiert (nach einem Update, 10 bis 20 s). Rot, wenn es in diesem Start nicht mit seiner Firmware läuft (nächster Versuch beim nächsten Einschalten), oder nach 5 Fehlversuchen in Folge: dann versucht es der KVM erst wieder, wenn seine Taste beim Einschalten gedrückt wird oder mit Firmware → Erneut versuchen.
diag-help-bt-fw = Das Modul läuft mit der Firmware, die der KVM mitbringt: ein Update des KVM aktualisiert es mit.
diag-help-bt-link-errors = Fehlgeschlagene Austausche mit dem Modul seit dem Start, sein Programmieren eingeschlossen.
diag-help-bt-hosts = Gekoppelte Hosts, die gerade verbunden sind und Eingaben empfangen können.

## Einstellungen auf der Kommandozeile

parse-colour = Farbe '{ $value }': #rrggbb verwenden
parse-hotkey = Tastenkürzel '{ $value }': { $error } (z. B. double:scrolllock oder ctrl+alt+1)
parse-host-name = Name '{ $value }': höchstens { $max } Bytes (leer: Host n)
parse-profile-name = Name '{ $value }': 1 bis { $max } Bytes
parse-mode = Modus '{ $value }': { $modes }
parse-profile-modes = '{ $value }': drei Modi für Tastatur, Maus und Weitere, z. B. auto,auto,passthrough
parse-number = '{ $value }': eine Zahl bis { $max }
parse-one-of = '{ $value }': erlaubt ist { $choices }
parse-unknown-setting = unbekannte Einstellung

## Das Fenster: Seitenleiste und Kopf

window-title-sim = { -product } (simuliert)
simulated-kvm = simulierter KVM
page-overview = Übersicht
page-ports = Anschlüsse
page-displays = Monitore
page-keyboard = Tastatur
page-settings = Einstellungen
page-firmware = Firmware
page-wireless = { -wireless }
light-mode = Helles Design
dark-mode = Dunkles Design
no-kvm-connected = Kein KVM verbunden
leds = LEDs
active-pc = Aktiver PC
profile = Profil
connected-to = Verbunden · { $device }
reading = Liest…
not-connected = Nicht verbunden

## Übersicht

chip-active = AKTIV
chip-this-computer = dieser Computer
pc-vbus = VBUS { $volts } V · bietet { $offers }
showing = Angezeigt
switch-here = Hierher wechseln
front-ports = Anschlüsse vorne
unnamed-device = (Gerät ohne Namen)
host-has-input = { $host } bekommt Tastatur und Maus über { -wireless(case: "prefix") }. Durchgereichte Anschlüsse bleiben bei { $pc }.
power = Stromversorgung
power-left = Über { $source }, etwa { $milliamps } mA übrig für die Anschlüsse vorne
budget-low = Die angeschlossenen Geräte brauchen vielleicht mehr: den PWR-Anschluss nutzen.
selftest-line = Selbsttest { $result }
recent = Zuletzt

## Anschlüsse

port-keyboard = Tastaturanschluss
port-mouse = Mausanschluss
port-other = Anschluss „Weitere“
port-short-keyboard = Tastatur
port-short-mouse = Maus
port-short-other = Weitere
redetect = Neu erkennen
mode = Modus
no-device = Kein Gerät
running-mode = Läuft als { $mode }
up-to-ma = bis { $milliamps } mA
over-current-faults = { $count ->
    [one] { $count } Überstromfehler
   *[other] { $count } Überstromfehler
}
profiles = Profile
profiles-note = Jedes Profil ist ein Satz Anschlussmodi, z. B. emulierte Geräte für die Arbeit und Durchreichen für Spiele. Ein langer Druck auf die Taste am KVM wählt das nächste (Seite Tastatur). Die Anschlüsse unten laufen mit dem aktiven Profil.
name = Name
profiles-in-use = Profile in Gebrauch:
remember-profile = Im zuletzt aktiven Profil starten (sonst in Profil 0, und das Wählen eines Profils schreibt nichts in den Flash des KVM)
config-locked = Die Konfiguration per Software ist am KVM ausgeschaltet.
config-locked-how = Wieder einschalten: Einstellungen → Steuerung durch Software, dann die Taste am KVM drücken.

## Tastatur

hotkeys = Tastenkürzel
hotkey-toggle = PC wechseln
hotkey-pc1 = PC1 wählen
hotkey-pc2 = PC2 wählen
hotkey-next = Nächstes Ziel
cant-learn-firmware = Die Firmware dieses KVM kann keine Tastenkürzel lernen: bitte eintippen
cant-learn-not-active = Nur der aktive PC kann ein Tastenkürzel lernen: zuerst den KVM auf diesen PC schalten
cant-learn-no-keyboard = Keine Tastatur angeschlossen, die der KVM liest
learning-another = Ein anderes Kürzel wird gerade gelernt
learned-with-warning = { $warning } „Übernehmen“ verwendet es trotzdem.
learned = Von der Tastatur gelernt. „Übernehmen“ verwendet es.
not-learned = Nicht gelernt: { $why }.
kvm-no-answer = Der KVM hat nicht geantwortet.
learn-hint-sim = Oder „Lernen“ drücken, dann das Kürzel in diesem Fenster: es spielt die Tastatur des simulierten KVM.
learn-hint = Oder „Lernen“ drücken, dann das Kürzel auf der Tastatur am KVM.
press-the-hotkey = Kürzel drücken… { $seconds } s
apply = Übernehmen
cancel = Abbrechen
learn = Lernen
learn-hover = Das Kürzel auf der Tastatur am KVM drücken, statt es einzutippen
passthrough-keyboard-note = Eine durchgereichte Tastatur geht direkt zum PC: der KVM sieht sie nicht, also wirkt kein Kürzel auf ihr, und keins wird von ihr gelernt.
double-tap-window = ms Fenster für Doppeltipp
kvm-key = Die Taste am KVM
tap = Tippen
long-press = Langer Druck
press-tap = tippen
press-long-from = langer Druck ab { $seconds } s
held-for-long-press = Gehalten für einen langen Druck:
kvm-key-note = Unabhängig von dieser Einstellung bestätigt ein Druck ein wartendes Firmware-Update, und im Ruhezustand weckt der erste Druck den KVM nur.
keymap-vial = Tastenbelegung (Vial)
keymap-note = Die Belegung mit der Vial-App bearbeiten, solange ihre Schnittstelle angezeigt wird. Die Belegung wirkt auch bei ausgeschalteter Schnittstelle. Ändern der Schnittstelle lässt beide PCs den KVM neu erkennen (etwa 1 s).
vial-interface = Vial-Schnittstelle:
vial-off = Aus
vial-auto = Automatisch (wenn kein gespiegeltes Gerät den Endpunkt braucht)
vial-always = Immer
apply-keymap = Die Belegung auf Tastaturen anwenden, die der KVM normalisiert

## Einstellungen

this-computer = Dieser Computer
autostart = Mit dem System starten, im Tray
could-not-change = Konnte es nicht ändern: { $error }
appearance = Darstellung:
theme-system = System
theme-dark = Dunkel
theme-light = Hell
language = Sprache:
language-system = System
brightness = Helligkeit
pc-colour = Farbe { $pc }
pc-side = PC-Seite
polling = Abfrage:
polling-125us = 125 µs (geringste Latenz)
polling-1ms = 1 ms (für wählerische BIOS)
power-cycle = Durchgereichte Geräte beim PC-Wechsel kurz stromlos schalten
standby = Wenn die PCs aus sind oder schlafen
standby-note = Hat kein PC den KVM 10 s lang benutzt, gehen seine LEDs aus und die Anschlüsse vorne werden abgeschaltet. Ein PC, der eingeschaltet wird, oder ein Druck auf die Taste am KVM weckt ihn.
auto-select = Dem eingeschalteten PC folgen: wird der aktive PC ausgeschaltet, wird der andere aktiv
standby-wake = Tastatur und Maus versorgt lassen, während ein PC schläft, damit ein Tastendruck ihn weckt
standby-glow = Die Taste im Ruhezustand leuchten lassen (sonst sind alle LEDs aus)
save-now = Jetzt speichern
factory-settings = Werkseinstellungen
unsaved-changes = Nicht gespeicherte Änderungen werden in wenigen Sekunden geschrieben.
software-control = Steuerung durch Software
software-control-note = Jeder PC kann diese ausschalten. Wieder einschalten geht nur mit der Taste am KVM: ein Druck innerhalb von { $seconds } s nach der Anfrage hier, oder die Taste beim Einschalten des KVM gedrückt halten.
allow-switching = Umschalten per Software erlauben
allow-config = Konfiguration per Software erlauben
not-turned-on = Nicht eingeschaltet: die Taste am KVM wurde nicht rechtzeitig gedrückt.
press-key-to-confirm = Zum Bestätigen die Taste am KVM drücken (noch { $seconds } s). Ihre LED pulsiert weiß.
confirm-lock-switching = Umschalten per Software ausschalten?
confirm-lock-config = Konfiguration per Software ausschalten?
turn-off = Ausschalten

## Firmware

versions = Versionen
wireless-module = { -wireless(case: "prefix") }-Modul
hardware = Hardware
serial = Seriennummer
uptime = Laufzeit
gave-up-note-chip = Der KVM hat nach 5 Fehlversuchen in Folge aufgehört, damit er den Chip nicht bei jedem Einschalten neu programmiert. Er versucht es wieder mit einer neuen Firmware, wenn seine Taste beim Einschalten gedrückt wird, oder jetzt:
try-again = Erneut versuchen (der KVM startet neu)
update = Update
image-file = Image-Datei:
install = Installieren
press-key-to-install = Zum Installieren des Updates die Taste am KVM drücken.
installed = Installiert; der KVM startet neu.
fw-file-title = Firmware-Datei
fw-file-hint = Ordner eines Release, kvm-rp2354.bin oder eine .uf2
browse = Durchsuchen…
fw-shipped = Mitgelieferte Firmware
fw-shipped-note = mit dieser App geliefert
fw-file-is = Firmware { $version } für Hardware { $major }.x
fw-same = Der KVM hat diese Version schon.
fw-newer = Neuer als die { $kvm } des KVM.
fw-older = Älter als die { $kvm } des KVM: Installieren geht zurück.
fw-no-file = Keine Firmware gewählt: eine Datei oder den Ordner eines Release wählen.
update-over-pc-note = Über den Anschluss dieses Computers, an den laufenden KVM. Die neue Firmware startet auf Probe: kommt sie nicht hoch, kehrt der KVM zur alten zurück.
pick-firmware = Firmware des KVM wählen
firmware-files = KVM-Firmware
boot-title = Im Bootloader (Anschluss PWR/SERVICE)
boot-kvm = { -product } mit Firmware { $version }; ein Update kommt in Partition { $slot }.
boot-kvm-empty = { -product } ohne Firmware; sie kommt in Partition { $slot }.
boot-new = Eine neue Platine, noch leer. Einrichten schreibt die Partitionstabelle und die Firmware.
boot-new-unpartitioned = Firmware { $version }, aber keine Partitionstabelle (mit einem Debug-Adapter geflasht). Einrichten schreibt beides.
boot-other = Kein { -product }: ein RP2350 mit anderer Partitionstabelle oder anderer Firmware. Darauf wird nichts installiert.
boot-unknown-note = Ohne das kann diese App einen { -product } nicht von einer neuen Platine unterscheiden: nur auf einem { -product } mit Firmware installieren und nur eine neue Platine einrichten.
boot-drive = Laufwerk: { $path }
boot-no-drive-note = Sein Laufwerk ist nicht eingehängt: Installieren hängt es ein, oder es zuerst im Dateimanager öffnen.
boot-set-up = Einrichten
boot-writes = Schreibt { $file }: Firmware { $version } für Hardware { $major }.x
boot-writing = schreibt
boot-restart = Ohne Installieren neu starten
boot-restart-hover = Verlässt den Bootloader: die Platine startet die Firmware, die sie hat.
boot-installed = Installiert: die Platine startet mit der neuen Firmware neu. Zum Benutzen an die PCs anschließen.
boot-none-title = Über den Anschluss PWR/SERVICE installieren
boot-none-note = Für eine neue Platine, oder eine, deren Firmware nicht startet: diesen Computer an den Anschluss PWR/SERVICE des KVM anschließen. Eine neue Platine erscheint hier von selbst; jede andere, wenn beim Einstecken ihr Taster im kleinen Loch gedrückt bleibt.
boot-found-hint = Ein { -product } wartet in seinem Bootloader am Anschluss PWR/SERVICE.
open-firmware-page = Seite Firmware
identify = Identifizieren (LEDs blinken)
diagnostics = Diagnose
measuring = Misst…
diag-bad = { $count } schlecht
diag-warnings = { $count ->
    [one] { $count } Warnung
   *[other] { $count } Warnungen
}
diag-all-good = alles gut
diag-readings = { $count } Messwerte, vor { $seconds } s
copy = Kopieren
copy-hover = Die Messwerte als Text kopieren
looking-for-kvm = Suche den KVM…
plug-in-kvm = Den KVM an diesen Computer anschließen.
udev-hint = Aus dem Quellcode installiert? Dann software/odkvmctl/udev/70-opendyn-kvm.rules installieren (DEVELOPMENT.md).

## Funk-Hosts (die Bluetooth-Funktion; ihr Name ist der Term -wireless)

host-connecting = verbindet…
host-hover = { -wireless }, { $state }
send = Senden
cancel-pairing = Kopplung abbrechen
host-pairing = Ein { -wireless(case: "prefix") }-Host koppelt sich
passkey-text = Der Host zeigt sechs Ziffern: diese auf der Tastatur am KVM eingeben und Enter drücken (Escape bricht ab), oder hier eingeben. Bis dahin tippt die Tastatur am KVM nirgendwo sonst.
module-is = Das Modul { $state }.
module-programming-note = Nach einem Firmware-Update programmiert der KVM das Modul mit der Firmware, die er mitbringt; das dauert 10 bis 20 s. { -wireless(case: "prefix") }-Hosts verbinden sich danach wieder.
module-failed-note = { -wireless } bleibt aus, bis der KVM wieder eingeschaltet wird; dann versucht er es noch einmal. Die beiden PCs funktionieren wie bisher.
gave-up-note-module = Der KVM hat nach 5 Fehlversuchen in Folge aufgehört, damit er das Modul nicht bei jedem Einschalten neu programmiert. Er versucht es wieder mit einer neuen Firmware, wenn seine Taste beim Einschalten gedrückt wird, oder jetzt:
pair-new-host = Neuen Host koppeln
pair-note = Macht den KVM für eine Minute als „{ -product }“ sichtbar. Ihn dann am PC, Tablet oder Telefon in den { -host-pairing-settings } als Tastatur koppeln. Zeigt der Host sechs Ziffern, diese auf der Tastatur am KVM eingeben und Enter drücken.
pair = Koppeln
slots-full = Zuerst einen Host entfernen: alle vier Plätze sind belegt
discoverable = Sichtbar · { $seconds } s
stop = Stopp
hosts = Hosts
hosts-note = { -wireless(case: "prefix") }-Hosts bekommen die eigene Tastatur und Maus des KVM (Spiegeln, Identität und Durchreichen bleiben bei den PCs). „Verbunden bleiben“ macht das Umschalten sofort; „Nur wenn gewählt“ passt zu Telefonen und Tablets, die ihre Bildschirmtastatur ausblenden, solange eine Tastatur verbunden ist.
slot-free = frei
active = aktiv
forget = Entfernen
forget-hover = Hebt die Kopplung auf. Um wieder zu koppeln, „{ -product }“ auch auf dem Host entfernen.
link-every = Alle { $ms } ms
link-may-skip = { $count ->
    [one] darf { $count } auslassen
   *[other] darf { $count } auslassen
}
link-signal = Signal { $dbm } dBm
policy-stay = Verbunden bleiben
policy-on-select = Nur wenn gewählt
a-host = ein { -wireless(case: "prefix") }-Host

## Tray

tray-not-connected = { -product }: nicht verbunden
tray-host-active = { -product }: { $host } aktiv ({ -wireless })
tray-pc-active = { -product }: { $pc } aktiv
tray-switch-to = Wechseln zu { $target }
tray-open = Öffnen
tray-quit = Beenden

## Letzte Ereignisse

ev-switched = Gewechselt zu { $target } ({ $source })
ev-port-changed = { $port } geändert
ev-fault-port = { $port }: { $fault }
ev-fault = Fehler: { $fault }
ev-setting-changed = Einstellung { $name } geändert
ev-hotkey-learned = Tastenkürzel gelernt: { $hotkey }
ev-hotkey-not-learned = Tastenkürzel lernen: { $why }
ev-pairing-open = { -wireless(case: "prefix") }-Kopplung offen
ev-pairing-closed = { -wireless(case: "prefix") }-Kopplung geschlossen
ev-passkey-wanted = Ein { -wireless(case: "prefix") }-Host fragt nach seinem Code
ev-paired = Gekoppelt: { $host } (Platz { $slot }; auf der Seite { -wireless } benennen)
ev-forgot = { -wireless(case: "prefix") }-Host { $host } entfernt
ev-host-connected = { $host } verbunden
ev-host-disconnected = { $host } getrennt
ev-switched-host = Gewechselt zu { $host } ({ -wireless })
ev-monitor-not-switched = Monitor nicht umgeschaltet: { $error }
fault-over-current = Überstrom
fault-enumeration = Erkennung fehlgeschlagen
fault-link-down = Verbindung unterbrochen
fault-power-budget = nicht genug Strom
fault-hub = Hub-Fehler
no-diagnostics = Die Firmware dieses KVM hat noch keine Diagnose: bitte aktualisieren.
kvm-refused = Der KVM hat abgelehnt: { $why }.

## Monitore, die dem KVM folgen (DDC/CI)

profile-n = Profil { $n }
display-follow = Folgt dem KVM
display-pc1 = Bleibt auf PC1
display-pc2 = Bleibt auf PC2
display-off = Nicht umgeschaltet
input-none-pc = — nicht umgeschaltet
input-none-host = — bleibt beim PC
display-switched = { $monitor } auf { $input } umgeschaltet
displays-title = Monitoreingänge folgen dem KVM
displays-note = Wechselt der KVM den PC (oder zu einem { -wireless(case: "prefix") }-Host) oder das Profil, schaltet dieser Computer die Monitore unten um (DDC/CI): in jedem Profil kann ein Monitor dem KVM folgen, auf einem PC bleiben oder in Ruhe gelassen werden. Der KVM meldet jede Änderung beiden PCs, also jeden Monitor möglichst nur auf einem PC einrichten.
displays-two-pcs = Die beiden PCs teilen diese Einstellungen nicht: ein auf beiden eingerichteter Monitor bekommt jeden Befehl doppelt, und folgt keiner der Einrichtungen, wenn sie sich unterscheiden. Ein PC genügt, wenn der Monitor auch auf einem nicht angezeigten Eingang hört: „PC1 zeigen“ / „PC2 zeigen“ weg von diesem PC und zurück zeigt es. Kommt er nicht zurück, ihn auf beiden PCs gleich einrichten.
displays-ddc-on = DDC/CI muss im Menü des Monitors eingeschaltet sein.
look-again = Erneut nach Monitoren suchen
asking-monitors = Frage die Monitore…
this-computer-is = Dieser Computer ist { $pc }
kvm-not-connected = KVM nicht verbunden
no-monitor = Kein Monitor hat geantwortet. Prüfen, ob DDC/CI im Menü des Monitors eingeschaltet ist.
saved-not-connected = Gespeichert, gerade nicht verbunden
input-other = anderer
input-other-hover = Ein Eingang, den der Monitor nicht auflistet, hexadezimal (z. B. 0x0F), dann Enter
use-current = Aktuellen nehmen
use-current-hover = Der Eingang, den dieser Monitor gerade zeigt
show-target = { $target } zeigen
monitor-shows = zeigt { $input }
not-answering = antwortet nicht
target-input = Eingang { $target }
displays-hosts-note = { -wireless(case: "prefix") }-Hosts sind freiwillig: solange einer Tastatur und Maus hat, zeigt ein Monitor, der dem KVM folgt, ihn, wenn er hier einen Eingang hat, und bleibt sonst beim PC.
in-each-profile = In jedem Profil:
input-missing = Ein Profil braucht einen Eingang, der oben nicht gewählt ist.
could-not-save = Konnte die Einstellungen dieses Computers nicht speichern: { $error }
input-unknown = Eingang { $value }
ddc-no-answer = keine Antwort (ist DDC/CI im Menü des Monitors eingeschaltet?)
ddc-garbled = gestörte Antwort
ddc-unexpected = unerwartete Antwort
ddc-unsupported-code = der Monitor unterstützt das nicht
ddc-not-connected = { $id }: nicht verbunden
ddc-unsupported-os = DDC/CI wird nur unter Windows und Linux unterstützt
ddc-no-access = { $path }: kein Zugriff. Das Paket installieren, oder die udev-Regel aus dem README, dann neu anmelden.
ddc-no-i2c-dev = { $path }: fehlt, das Modul i2c-dev ist nicht geladen. Das Paket installieren, oder es laden, wie im README beschrieben.

## Verbindung zum KVM

error-timeout = der KVM hat nicht geantwortet
error-device = der KVM hat abgelehnt: { $why }
error-protocol = Protokollfehler: { $error }
protocol-unexpected = unerwartete Antwort { $answer }
protocol-update-ended = Update endete im Zustand { $state }
error-not-confirmed = die Taste am KVM wurde nicht rechtzeitig gedrückt: nichts geändert
host-n = Host { $n }
hid-open-linux = { $error } (software/odkvmctl/udev/70-opendyn-kvm.rules installieren für Zugriff ohne root)
no-kvm-found = kein { -product } gefunden

## Firmware-Dateien

img-damaged-uf2 = beschädigte UF2-Datei
img-uf2-partition-table = diese UF2 enthält auch die Partitionstabelle (kvm-first.uf2); kvm-rp2354.uf2 oder kvm-rp2354.bin verwenden
img-uf2-other-chip = UF2 für einen anderen Chip (Familie { $family })
img-uf2-out-of-range = UF2-Block bei { $address } außerhalb des Bereichs
img-too-big = { $kb } KB ist mehr, als eine Firmware-Partition fasst ({ $max } KB)
img-not-rp2354b = keine RP2354B-Firmware (keine RP2350-Image-Definition)
img-wrong-kind = RP2350-Image der falschen Art (Flags { $flags })
img-no-fw-id = keine { -product }-Firmware (keine Firmware-ID)
img-other-hardware = Firmware { $version } ist für Hardware { $major }.x, dieser KVM ist Hardware { $hw }
img-not-first = keine UF2 zur Erstinstallation (kvm-first.uf2: Partitionstabelle und Firmware)
fw-read = { $file }: { $error }
fw-not-in-folder = kein { $name } in { $folder }
boot-no-picoboot = das Boot-ROM zeigt keine PICOBOOT-Schnittstelle
boot-no-permission = keine Berechtigung für das Boot-ROM des RP2350 (USB 2e8a:000f): die udev-Regel installieren (software/odkvmctl/udev/70-opendyn-kvm.rules)
boot-no-access = das Boot-ROM des RP2350 lässt sich nicht öffnen: { $error }
boot-usb = USB: { $error }
boot-rom-ignored = das Boot-ROM hat die Datei ignoriert (nicht für die Partitionen dieser Platine)
boot-rom-locked = ein anderes Programm hält das Boot-ROM (picotool?)
boot-rom-bad-address = das Boot-ROM hat abgebrochen: eine Adresse außerhalb des Flash
boot-rom-write-error = das Boot-ROM konnte den Flash nicht schreiben
boot-rom-reboot-failure = das Boot-ROM hat die Firmware geschrieben, konnte sie aber nicht starten
boot-no-drive = das Laufwerk des Boot-ROM (RP2350) ist nicht eingehängt: im Dateimanager öffnen, dann erneut installieren
boot-write = Schreiben auf das Laufwerk des Boot-ROM: { $error }
boot-no-restart = die Platine ist nach dem Kopieren nicht neu gestartet (20 s nichts)

## odkvmctl: Hilfe

cli-about = Den { -product }-Umschalter steuern
cli-sim = Mit einem eingebauten simulierten KVM statt dem echten sprechen
cli-status = Den Zustand des KVM zeigen (Standard)
cli-diag = Takte, Spannungen, Temperaturen und USB-Verbindungen des KVM messen, jeweils gut, Warnung oder schlecht; Rückgabewert 1, wenn etwas schlecht ist
cli-list = Die Steuerschnittstellen von KVMs auflisten, die dieser Computer sieht
cli-watch = Ereignisse (Wechsel, Anschlussänderungen, Fehler) ausgeben, sobald sie geschehen
cli-switch = Tastatur, Maus und weiteres Gerät zu einem PC schalten, oder (mit dem { -wireless(case: "prefix") }-Modul) zu einem { -wireless(case: "prefix") }-Host
cli-wireless = { -wireless(case: "prefix") }-Hosts (KVMs mit dem Modul): auflisten (Standard), einen neuen koppeln, entfernen, benennen, oder festlegen, was mit einem Host passiert, während ein anderes Ziel aktiv ist
cli-port = Einen Anschluss vorne zeigen, seinen Modus setzen oder sein Gerät neu erkennen
cli-profile = Die Profile auflisten, eines aktiv machen (profile next, profile 1, profile gaming) oder eines ändern (profile 1 name gaming, profile 1 mouse passthrough)
cli-profile-which = Nummer oder Name eines Profils, oder next
cli-profile-setting = name, oder ein Anschluss: keyboard, mouse, other
cli-hotkey = Ein Tastenkürzel zeigen oder setzen: double:scrolllock, double:rctrl:noswallow, ctrl+alt+1, none; oder es mit learn von den nächsten Tasten auf der Tastatur am KVM lernen
cli-config = Einstellungen lesen und ändern
cli-identify = Die LEDs des KVM blinken lassen
cli-update = RP2354B-Firmware installieren: auf eine Platine, die im Bootloader am Anschluss PWR/SERVICE wartet, falls es eine gibt, sonst über einen PC-Anschluss (verlangt einen Druck auf die Taste am KVM)
cli-reboot = Den KVM neu starten, oder einen Chip in seinen Bootloader versetzen
cli-display = Monitore, deren Eingang dieser Computer per DDC/CI umschalten kann (kein KVM nötig)
cli-display-list = Die Monitore, der Eingang, den jeder zeigt, und die Eingänge, die er bietet (Standard; dauert ein paar Sekunden)
cli-display-input = Einen Monitor auf einen Eingang schalten, z. B. display input XYZ-1A2B-1234 0x11 (IDs aus display list)
cli-hosts-list = Die vier Host-Plätze: Name, Zustand, Verhalten
cli-hosts-pair = Den KVM 60 s lang als „{ -product }“ sichtbar machen; dann am PC, Tablet oder Telefon koppeln. Zeigt der Host sechs Ziffern, diese auf der Tastatur am KVM eingeben und Enter drücken (oder passkey verwenden)
cli-hosts-pair-stop = stop schließt die Kopplung vorzeitig
cli-hosts-passkey = Die sechs Ziffern, die ein koppelnder Host zeigt, wenn die Tastatur am KVM nicht zur Hand ist
cli-hosts-forget = Die Kopplung eines Hosts aufheben: Platz 1-4, oder all
cli-hosts-name = Einen Host-Platz benennen (leer: „Host n“)
cli-hosts-policy = stay: der Host bleibt verbunden, und der Wechsel zu ihm ist sofort. on-select: nur verbunden, solange er gewählt ist (für Telefone und Tablets, die ihre Bildschirmtastatur ausblenden, solange eine Tastatur verbunden ist)
cli-port-redetect = Das Gerät neu erkennen (nötig nach dem Tausch eines Geräts an einem durchgereichten Anschluss)
cli-config-get = Eine Einstellung zeigen, oder alle
cli-config-set = Eine Einstellung ändern (Namen und aktuelle Werte zeigt config get)
cli-config-save = Die Einstellungen jetzt in den Flash des KVM schreiben (sonst 5 s nach der letzten Änderung)
cli-config-reset = Zurück auf Werkseinstellungen (die Software-Sperren bleiben)
cli-target-next = Das nächste Ziel: PC1, PC2, dann jeder verbundene { -wireless(case: "prefix") }-Host
cli-reboot-at32-retry = Den AT32F405 bis zu 5 weitere Programmierversuche geben, dann neu starten
cli-reboot-wireless-retry = Dem { -wireless(case: "prefix") }-Modul bis zu 5 weitere Programmierversuche geben, dann neu starten

## odkvmctl: Ausgabe

cli-port-auto = automatisch: { $why }
cli-port-set-to = eingestellt auf { $mode }: { $why }
cli-status-head = { -product } ({ $device }): Firmware { $firmware }, AT32 { $at1 } / { $at2 }, Hardware { $hardware }, Seriennummer { $serial }
cli-module-firmware = { -wireless(case: "prefix") }-Modul: Firmware { $version }
cli-module-state = { -wireless(case: "prefix") }-Modul: { $state }
cli-active-host = Aktiv: { -wireless(case: "prefix") }-Host { $slot } „{ $host }“ (Heim-PC { $pc })
cli-active = Aktiv: { $target }
cli-this-computer = dieser Computer ist { $pc }
cli-profile-line = Profil: { $n } ({ $name })
cli-host-slot = Host { $slot }
cli-pairing-open = { -wireless(case: "prefix") }-Kopplung ist offen (noch { $seconds } s)
cli-passkey-waits = ein koppelnder Host wartet auf seinen Code: auf der Tastatur am KVM eingeben, dann Enter
cli-budget-low = Geräte brauchen vielleicht mehr
cli-power = Strom: { $power }
cli-selftest = Selbsttest: { $result }
cli-try-again = erneut versuchen: { $command }, oder die Taste am KVM beim Einschalten gedrückt halten
cli-ports = Anschlüsse:
cli-switching-disabled = Umschalten per Software ist ausgeschaltet
cli-config-disabled = Konfiguration per Software ist ausgeschaltet
cli-locked = Am Gerät gesperrt: { $what }
cli-host-ready = verbunden, { $ms } ms
cli-profiles-in-use = { $count } von { $max } in Gebrauch: odkvmctl config set profiles <1-{ $max }>
cli-no-profile = kein Profil '{ $which }': eine Zahl von 0 bis { $last } oder ein Name
cli-learn-sim = die Tastatur des simulierten KVM lässt sich nicht von hier drücken: „Lernen“ in `opendyn-kvm --sim` versuchen
cli-learn-press = Das neue Kürzel innerhalb von { $seconds } s auf der Tastatur am KVM drücken. Die Tasten erreichen solange keinen PC.
cli-learn-unsupported = die Firmware dieses KVM kann keine Tastenkürzel lernen: das Kürzel als Text angeben, z. B. ctrl+alt+1
cli-nothing-changed-why = { $why }: nichts geändert
cli-use-it-for = Für { $name } verwenden? [j/N]
cli-turn-it-off = Ausschalten? [j/N]
## Wörter außer y und yes, die eine [j/N]-Frage mit ja beantworten, durch Kommas getrennt.
cli-yes-words = j, ja
cli-readings = { $count } Messwerte: { $verdict }
cli-ev-back-on-pc = Eingabe wieder bei einem PC
cli-ev-host-active = gewechselt zu { -wireless(case: "prefix") }-Host { $slot }
cli-ev-pairing-changed = { -wireless(case: "prefix") }-Kopplung geändert
cli-ev-host-changed = { -wireless(case: "prefix") }-Host { $slot } geändert
cli-no-monitors = keine Monitore gefunden
cli-inputs = Eingänge: { $inputs }
cli-input-number = Eingang '{ $value }': eine Zahl wie 0x11 oder 17
cli-no-interface = keine { -product }-Steuerschnittstelle gefunden
cli-connecting = Verbindung zum KVM
cli-watching = beobachte { $device } (Strg+C beendet)
cli-no-module = dieser KVM hat kein { -wireless(case: "prefix") }-Modul
cli-no-host-in-slot = auf diesem Platz ist kein Host gekoppelt
cli-no-module-boards = dieser KVM hat kein { -wireless(case: "prefix") }-Modul (Platinen ab 0.16.0 können eines tragen)
cli-pair-usage = '{ $value }': `wireless pair` oder `wireless pair stop` verwenden
cli-pairing-opened = Die Kopplung ist { $seconds } s offen: „{ -product }“ am PC, Tablet oder Telefon koppeln.
cli-pairing-digits = Zeigt er sechs Ziffern, diese auf der Tastatur am KVM eingeben und Enter drücken.
cli-six-digits = die sechs Ziffern, die der Host zeigt
cli-no-passkey-wanted = kein Host wartet auf einen Code
cli-slot-or-all = Platz 1-4 oder all
cli-slot = Platz 1-4
cli-no-profiles = diese Firmware hat keine Profile
cli-profile-not-in-use = Profil { $n } ist nicht in Gebrauch: odkvmctl config set profiles { $count }
cli-missing-value = Profil { $setting }: der Wert fehlt
cli-nothing-changed = Nichts geändert.
cli-unknown-setting = unbekannte Einstellung '{ $name }'
cli-press-key-within = Zum Bestätigen innerhalb von { $seconds } s die Taste am KVM drücken…
cli-saved = gespeichert
cli-factory-restored = Werkseinstellungen wiederhergestellt
cli-reading = lese { $file }
cli-firmware-for = Firmware { $version } für Hardware { $major }.x
cli-update-file = Die Firmware: Ordner eines Release, kvm-rp2354.bin oder eine .uf2 (Standard: die mit diesem Programm gelieferte Firmware)
cli-boot-no-drive = Laufwerk nicht eingehängt
cli-no-firmware = keine Firmware angegeben, und keine mit diesem Programm geliefert
cli-no-kvm-anywhere = kein { -product } an einem PC-Anschluss, und keiner im Bootloader am Anschluss PWR/SERVICE
cli-boot-several = mehrere Platinen im Bootloader: eine nach der anderen anschließen
cli-boot-unknown = Ohne das kann odkvmctl einen KVM nicht von einer neuen Platine unterscheiden. kvm-rp2354.uf2 (ein KVM mit Firmware) oder kvm-first.uf2 (eine neue Platine) selbst auf sein Laufwerk RP2350 kopieren.

## Bildschirmsprung (der Name der Funktion ist der Term -hop)

page-screens = { -hop }
switch-source-edge = der Zeiger hat einen Bildschirmrand überquert
hop-line = { -hop }: { $state }
hop-state-off = in diesem Profil aus
hop-state-active = an
hop-state-no-absolute = braucht den absoluten Zeiger
hop-state-not-normalized = braucht den Mausanschluss im Modus Normalisiert
hop-state-no-screens = dieser PC hat keinen Bildschirm in der Anordnung
hop-state-boot-protocol = aus, solange der PC im Firmware-Setup ist
hop-state-wireless = aus, solange ein { -wireless(case: "prefix") }-Host die Eingabe hat
accel-flat = Linear (nur Geschwindigkeit)
accel-precision = Präzisionskurve wie unter Windows
accel-adaptive = Adaptiv wie unter Linux
mapping-desktop = Über alle seine Bildschirme (Linux)
mapping-main-screen = Nur auf seinem Hauptbildschirm (Windows)
units-physical = physische Pixel (Windows, X11)
units-logical = logische Pixel (Wayland)
parse-screen = '{ $value }': ein Bildschirm wird pc1 2560x1440 150% at 1920,0 desk 0,0 geschrieben (Skalierung und Positionen dürfen fehlen; mit no-hop geht der Zeiger nicht darauf, mit fixed bleibt seine Stelle auf seinem Computer), oder off
parse-range = '{ $value }': eine Zahl von { $min } bis { $max }
screens-none-found = keine Bildschirme gefunden
screens-xrandr-missing = xrandr lässt sich nicht starten, um die Bildschirme aufzulisten ({ $error })
screens-xrandr-failed = xrandr konnte die Bildschirme nicht auflisten: { $error }
screens-unsupported-os = Bildschirme werden nur unter Windows und Linux gefunden
profile-hop = Sprung
profile-hop-hover = { -hop } in diesem Profil (eigene Seite)
hop-firmware-too-old = Die Firmware dieses KVM hat keinen { -hop }: bitte aktualisieren.
hop-what = Der KVM bewegt den Zeiger selbst, über die unten angeordneten Bildschirme. Wird er über einen Rand auf einen Bildschirm des anderen PCs geschoben, schaltet der KVM auf diesen PC um, samt Tastatur.
hop-turn-on-absolute = Absoluten Zeiger einschalten
hop-mouse-normalized = Mausanschluss in diesem Profil normalisiert betreiben
hop-in-profiles = An in diesen Profilen
hop-profile-active = { $name } (aktiv)
hop-absolute-pointer = Absoluter Zeiger an der Maus des KVM
hop-absolute-pointer-note = Der { -hop } braucht ihn. Ein- oder Ausschalten lässt beide PCs die Geräte des KVM einmal neu erkennen.
hop-arrangement = Anordnung
hop-arrangement-note = Die Bildschirme dorthin ziehen, wo sie stehen. Der Zeiger wechselt, wo zwei Bildschirme aneinanderstoßen, und springt über eine Lücke dazwischen; ein Rand ohne Nachbarn hält ihn auf.
hop-overlap = Einige Bildschirme überlappen sich: zwischen ihnen kann der Zeiger nicht wechseln. Ein Monitor an beiden PCs: auf dem PC, der ihn nicht zeigt, den { -hop } für ihn ausschalten und ihn auf den Bildschirm des anderen PCs legen.
hop-add-screen = Bildschirm für { $pc } hinzufügen
hop-detect = Bildschirme von { $pc } erkennen
hop-detect-hover = Die Bildschirme dieses Computers, mit Auflösung, Skalierung und Lage in seinen Anzeigeeinstellungen
hop-remove-screen = Entfernen
hop-draft = Noch nicht übernommen: Der KVM behält seine Anordnung, bis Sie übernehmen.
hop-applying = Übernommen: Warten auf die Bestätigung des KVM. Lehnt er ab, bleiben die Änderungen hier für einen neuen Versuch.
hop-apply = Übernehmen
hop-discard = Änderungen verwerfen
hop-mapping-of = { $pc } bildet den Zeiger ab
hop-units-of = Einheiten der Anzeigeeinstellungen von { $pc }
hop-mapping-note = Windows legt einen absoluten Zeiger nur auf seinen Hauptbildschirm. Seine anderen Bildschirme erreicht der KVM, indem er den Mauszeiger wie eine Maus dorthin bewegt, und Windows beschleunigt das: für genaue Ränder auf so einem PC „Zeigerbeschleunigung verbessern“ ausschalten oder { -product } dort laufen lassen.
hop-mapping-windows = Auf diesem Computer läuft Windows, das den absoluten Zeiger nur auf seinen Hauptbildschirm legt: über alle Bildschirme landet der Zeiger an der falschen Stelle, und die anderen Bildschirme sind nicht erreichbar. Wählen Sie „Nur auf seinem Hauptbildschirm (Windows)“.
hop-no-screens = Noch keine Bildschirme: für jeden PC einen hinzufügen.
hop-main-screen = Hauptbildschirm
hop-screen-hover = Klicken zum Bearbeiten (erneut klicken für einen Bildschirm darunter), ziehen zum Verschieben
hop-screen-pc = Computer
hop-screen-resolution = Auflösung
hop-screen-scale = Skalierung
hop-screen-desk = Lage auf seinem Computer
hop-screen-at = Lage hier
hop-screen-used = Der { -hop } geht darauf
hop-screen-used-note = Aus für einen Monitor an beiden PCs, der den anderen zeigt: ihn auf dessen Bildschirm legen. Sein PC zählt ihn weiterhin, sodass der Zeiger auf seinen anderen Bildschirmen an der richtigen Stelle landet.
hop-screen-off-short = ohne { -hop }
hop-screen-desk-note = Wo der Bildschirm in den Anzeigeeinstellungen seines Computers liegt, in dessen Pixeln (logischen auf einem Wayland-Desktop). Unter Windows liegt der Hauptbildschirm bei 0, 0.
hop-detected = { $count ->
    [one] { $pc } hat jetzt 1 Bildschirm, wie dieser Computer.
   *[other] { $pc } hat jetzt { $count } Bildschirme, wie dieser Computer.
} { $pc } bildet den Zeiger ab: { $mapping }.
hop-pointer = Zeiger
hop-accel = Beschleunigung
hop-speed = Geschwindigkeit
hop-resistance = Randwiderstand
hop-corner = Ecken
hop-pointer-note = Die PCs beschleunigen einen absoluten Zeiger nicht, also tut es der KVM. Der Randwiderstand ist, wie weit der Zeiger gegen einen Rand geschoben werden muss, bevor er zum anderen PC wechselt; die Ecken sind, wie weit von jedem Ende dieses Rands er nie wechselt, damit die Ecken leicht zu treffen bleiben. Beim Ziehen wechselt er nie.
cli-screens = Bildschirmsprung: die Anordnung der Bildschirme beider PCs und wie sich der Zeiger bewegt
cli-screens-list = Anordnung und Zeigereinstellungen anzeigen
cli-screens-detect = Die Bildschirme dieses Computers in die Anordnung übernehmen
cli-screens-which-pc = der KVM sagt nicht, welcher PC dieser Computer ist (Simulator oder Service-Anschluss): die Bildschirme stattdessen mit odkvmctl config set screen0 … einstellen
cli-hop-mapping = { $pc } bildet den Zeiger ab: { $mapping }
cli-hop-units = Die Anzeigeeinstellungen von { $pc } zählen in { $units }
cli-hop-pointer = Zeiger: { $accel }, Geschwindigkeit { $speed }, Randwiderstand { $resistance } px, Ecken { $corner } px
hop-out-of-date = Die Anzeigeeinstellungen dieses Computers weichen von der Kopie seiner Bildschirme im KVM ab; deshalb landet der Zeiger hier an der falschen Stelle: seine Bildschirme neu erkennen.
cli-hop-out-of-date = { hop-out-of-date } (odkvmctl screens detect)
