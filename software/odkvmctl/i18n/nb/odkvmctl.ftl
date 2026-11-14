# OpenDynKVM: odkvmctl og opendyn-kvm, norsk bokmål. Mangler en melding her, vises den engelske.

## Merker og navn. Navn på produktet og funksjoner er termer: endre dem her, så følger alle meldingene.
## -wireless er Bluetooth-funksjonen. Så lenge produktet ikke er listet hos Bluetooth SIG, heter den «trådløs»;
## sett deretter alle formene til «Bluetooth» ([prefix] og [prefix-title] til «Bluetooth-», de limes på neste ord:
## «trådløsmodul»; [adverb] til «via Bluetooth»).

-product = OpenDynKVM
-wireless = { $case ->
   *[title] Trådløst
    [lower] trådløs
    [plural] trådløse
    [prefix] trådløs
    [prefix-title] Trådløs
    [adverb] trådløst
}
## Der en telefon eller PC kobler til et tastatur: enhetens egen meny, som beholder sitt navn.
-host-pairing-settings = Bluetooth-innstillinger
-hop = { $case ->
   *[title] Skjermhopp
    [lower] skjermhopp
}

product-name = { -product }

## Vanlige ord

yes = ja
no = nei
on = på
off = av

## Feil KVM-en svarer med

err-unknown = ukjent forespørsel
err-malformed = feilformet forespørsel
err-locked = slått av på KVM-en (slå det på igjen med `odkvmctl config set software-config on` og et trykk på KVM-ens knapp, eller hold knappen inne mens KVM-en slås på)
err-busy = opptatt, prøv igjen
err-unsupported = støttes ikke av denne fastvaren
err-invalid-value = ugyldig verdi
err-needs-key-press = trykk på knappen på KVM-en for å bekrefte
err-no-device = ingen enhet på den porten
err-not-active = bare den aktive PC-en kan gjøre det: bytt KVM-en til denne PC-en først

## Lære en hurtigtast

learn-learned = lært
learn-timed-out = ingen taster som danner en hurtigtast ble trykket i tide
learn-cancelled = stoppet (KVM-en byttet PC, eller læringen ble avbrutt)
hotkey-warn-twin = Det er også hurtigtasten «{ $name }»: ett trykk ville gjort begge deler.
hotkey-warn-alone = { $key } alene ville byttet, og PC-ene ville ikke lenger fått den.

## Slå av styring fra programvare

lock-warn-switching = Ingen av PC-ene vil kunne bytte KVM-en fra programvare: verken med odkvmctl, appen eller ikonet i systemstatusfeltet. Knappen på KVM-en og hurtigtastene fungerer fortsatt.
lock-warn-config = Ingen av PC-ene vil kunne endre noen innstilling fra programvare: ingen portmoduser, profiler, hurtigtaster, fastvareoppdateringer eller omstarter, og Vial blir skrivebeskyttet. Statusen er fortsatt synlig.
lock-warn-back-on = Å slå det på igjen krever knappen på KVM-en: et trykk innen { $seconds } s etter at du ba om det, eller knappen holdt inne mens KVM-en slås på.

## Forbindelse til en PC

link-unpowered = av
link-detached = ikke tilkoblet
link-enumerating = kobler til
link-connected = tilkoblet
link-suspended = i hvilemodus
link-down = forbindelsen er brutt

## Porter foran

port-state-empty = tom
port-state-probing = gjenkjenner
port-state-hosted = aktiv
port-state-passthrough = gjennomkobling
port-state-unpowered = uten strøm
port-state-fault = feil
port-state-unsupported = støttes ikke
port-state-module = { -wireless(case: "prefix") }modul

mode-auto = Automatisk
mode-normalized = Normalisert
mode-mirrored = Speilet
mode-mirrored-identity = Speilet + identitet
mode-passthrough = Gjennomkobling (følger den aktive PC-en)
mode-passthrough-pc1 = Gjennomkobling, låst til PC1
mode-passthrough-pc2 = Gjennomkobling, låst til PC2
mode-short-auto = Auto
mode-short-normalized = Normalisert
mode-short-mirrored = Speilet
mode-short-identity = Identitet
mode-short-passthrough = Gjennomkobling
mode-short-pinned-pc1 = Låst PC1
mode-short-pinned-pc2 = Låst PC2

reason-not-auto = valgt av deg
reason-fits-normalized = alle inndata passer i de normaliserte grensesnittene
reason-extra-inputs = har inndata de normaliserte grensesnittene ikke kan bære
reason-other-port-hid = HID-enhet på porten Annet
reason-non-hid = har grensesnitt som ikke er HID
reason-xinput = XInput-spillkontroll
reason-endpoint-budget = ikke nok endepunkter igjen til å speile den
reason-identity-taken = en annen port bruker allerede identiteten

speed-low = low-speed
speed-full = full-speed
speed-high = high-speed

class-keyboard = tastatur
class-mouse = mus
class-consumer = medietaster
class-gamepad = spillkontroll
class-xinput = XInput
class-vendor-hid = produsentens inndata
class-vendor-channel = produsentens kommandoer
class-other-hid = annen HID
class-audio = lyd
class-video = video
class-storage = lagring
class-hub = hub

## Knappen på KVM-en

key-action-none = Ingenting
key-action-toggle-pc = Bytt til den andre PC-en
key-action-next-profile = Neste profil
key-action-other-port = Gå gjennom modusene til porten Annet
key-action-next-target = Neste mål (PC-ene, deretter { -wireless(case: "plural") } verter)

## Strøm

adv-none = ingen
adv-default = USB-standard
adv-1a5 = 1,5 A
adv-3a = 3 A
power-source-pwr = PWR-porten
power-source-pc = PC-ens USB

## Fastvareoppdatering

fw-state-idle = venter
fw-state-receiving = laster opp
fw-state-awaiting-key = trykk på knappen på KVM-en for å installere
fw-state-writing = installerer
fw-state-done = ferdig
fw-state-failed = mislyktes

## Den trådløse modulen

bt-module-up = i drift
bt-module-starting = starter
bt-module-programming = oppdateres
bt-module-failed = kjører ikke fastvaren sin: { -wireless(case: "lower") } av til neste gang den slås på
bt-module-gave-up = ga opp etter 5 forsøk: { -wireless(case: "lower") } av
bt-module-absent = ikke montert

## Selvtest

selftest-hub0 = Hub-0 (porter foran) ble ikke funnet
selftest-link1 = ingen forbindelse til PC1-brikken
selftest-link2 = ingen forbindelse til PC2-brikken
selftest-hub1 = PC1-huben virker ikke
selftest-hub2 = PC2-huben virker ikke
selftest-port-kb = strømbryteren til tastaturporten
selftest-port-ms = strømbryteren til museporten
selftest-port-ot = strømbryteren til porten Annet
selftest-power = målingene på PWR-porten er usannsynlige
selftest-at32-fw1 = programmering av PC1-brikken mislyktes (prøves igjen neste gang den slås på)
selftest-at32-fw2 = programmering av PC2-brikken mislyktes (prøves igjen neste gang den slås på)
selftest-at32-gave-up1 = ga opp å programmere PC1-brikken etter 5 forsøk
selftest-at32-gave-up2 = ga opp å programmere PC2-brikken etter 5 forsøk
selftest-at32-xtal1 = krystallen til PC1-brikken svinger ikke (ingen USB til PC1)
selftest-at32-xtal2 = krystallen til PC2-brikken svinger ikke (ingen USB til PC2)
selftest-bt-fw = { -wireless(case: "prefix") }modulen kjører ikke fastvaren sin (prøves igjen neste gang den slås på)
selftest-bt-gave-up = ga opp å programmere { -wireless(case: "prefix") }modulen etter 5 forsøk
selftest-running = kjører
selftest-passed = bestått
selftest-running-so-far = kjører, så langt: { $failed }
selftest-failed = MISLYKTES: { $failed }

## Hvorfor KVM-en byttet

switch-source-key = knappen på KVM-en
switch-source-hotkey = hurtigtast
switch-source-software = programvare
switch-source-boot = oppstart
switch-source-power = den aktive PC-en ble slått av

## Diagnostikk

diag-group-system = System
diag-group-clocks = Klokker
diag-group-usb = USB
diag-group-power = Strøm
diag-group-pc-chips = Brikker på PC-siden
diag-group-wireless = { -wireless }
diag-group-other = Annet

diag-uptime = Oppetid
diag-reset-cause = Siste omstart
diag-rp-temp = Temperatur RP2354B
diag-selftest = Selvtest
diag-dropped = Forkastede utdata fra motoren
diag-fw-updates = Fastvareoppdateringer over USB
diag-xosc = Krystall (XOSC)
diag-pll-sys = System-PLL
diag-pll-usb = USB-PLL
diag-rosc = Ringoscillator
diag-at-clock = Klokke AT32F405 nr. { $n }
diag-hub0-speed = Hub-0-forbindelse
diag-hub0-up = Hub-0 gjenkjent
diag-phy-swap = D+/D−-bytte (errata E1)
diag-kb-speed = Enhet på tastaturporten
diag-ms-speed = Enhet på museporten
diag-ot-speed = Enhet på porten Annet
diag-pc-speed = USB-hastighet { $pc }
diag-pc-link = Forbindelse { $pc }
diag-pc-resets = Bussnullstillinger { $pc }
diag-link-errors = SPI-feil AT32F405 nr. { $n }
diag-hub0-errors = Hub-0-endringer som ikke ble håndtert
diag-enum-fails = Mislykkede enumereringer på frontportene
diag-vbus = VBUS { $port }
diag-cc = CC { $port }
diag-at-3v3 = 3,3 V ved AT32F405 nr. { $n }
diag-budget = Strømbudsjett foran
diag-kb-faults = Feil på tastaturporten
diag-ms-faults = Feil på museporten
diag-ot-faults = Feil på porten Annet
diag-at-temp = Temperatur AT32F405 nr. { $n }
diag-at-fw = Fastvare AT32F405 nr. { $n }
diag-bt-module = ESP32-C3-modul
diag-bt-fw = ESP32-C3-fastvare
diag-bt-link-errors = ESP32-C3 UART-feil
diag-bt-hosts = Tilkoblede { -wireless(case: "plural") } verter
diag-unknown = element { $id }

reset-software = omstart fra programvare
reset-power-on = påslag
reset-brown-out = underspenning (forsyningen sank)
reset-glitch = forstyrrelse i forsyningen
reset-run-pin = RUN-pinnen
reset-watchdog = vakthund (også etter en oppdatering)
reset-debugger = feilsøker
reset-other = årsak { $cause }
hz-stopped = 0 Hz (stoppet)
clock-crystal = 12 MHz-krystall
clock-internal = intern oscillator: ingen USB
swap-found = { $state }, oppdaget under drift
fw-updates-on = på, fastvare i partisjon { $partition }
fw-updates-no-table = av: ingen partisjonstabell (programmert med en feilsøkingsprobe)
fw-updates-read-failed = av: oppstarts-ROM-en ga ikke partisjonstabellen ({ $detail })
fw-updates-other-table = av: en annen partisjonstabell ({ $detail })
fw-updates-other-layout = av: partisjonene avviker fra denne fastvarens ({ $detail })
fw-updates-unknown-partition = av: ukjent hvilken partisjon som kjører ({ $detail })
state-other = tilstand { $state }

diag-help-reset-cause = Oransje etter underspenning eller en forstyrrelse i forsyningen: 5 V sank. Tilsiktede omstarter har ingen farge.
diag-help-rp-temp = Oransje fra 70 °C, rød fra 85 °C.
diag-help-selftest = Rød når en del av KVM-en ikke virker; oransje for strømbryteren til en port foran.
diag-help-dropped = Arbeid fastvaren ikke hadde plass til. Oransje når ikke 0.
diag-help-fw-updates = Om KVM-en tar imot fastvareoppdateringer over en PC-port. Oransje når den ikke gjør det: koden i parentes sier hva kontrollen leste, nyttig i en feilrapport. Ingen farge på et kort programmert med en feilsøkingsprobe.
diag-help-xosc = 12 MHz-krystallen; rød når den ikke svinger.
diag-help-clk-sys = Målt mot krystallen. Rød ved 1 % avvik, eller når den ikke er 10 % over clk_usb (RP2350-E12).
diag-help-clk-usb = Målt mot krystallen. Rød ved mer enn 0,25 % avvik fra 48 MHz (USB full-speed).
diag-help-clk-adc = Rød ved mer enn 1 % avvik fra 48 MHz.
diag-help-at-clock = Rød når brikken kjører uten krystallen sin: da har den ingen USB til sin PC.
diag-help-hub0-speed = Huben til portene foran. Rød på low-speed: D+ og D− er krysset for PHY-ens innstilling (errata E1).
diag-help-phy-swap = Om USB-en til RP2354B bytter D+ og D− (errata E1): på for kortene 0.8 til 0.12.
diag-help-pc-speed = Oransje på full-speed: avspørring hvert 125 µs krever high-speed. Prøv en annen kabel eller port.
diag-help-pc-link = Rød når brikken på PC-siden ikke svarer; oransje så lenge PC-en ikke har satt den opp.
diag-help-link-errors = Mislykkede utvekslinger med brikken på PC-siden siden oppstart, etter at den har svart første gang.
diag-help-hub0-errors = Portendringer Hub-0 meldte som KVM-ens USB-vert ikke håndterer, nullstilt så de ikke skjuler de andre portene. Høyst noen få; flere er verdt en feilrapport.
diag-help-enum-fails = Enheter på frontportene KVM-en ikke kunne lese, hver prøvd igjen noen ganger ved å slå porten av og på. En enhet som trekkes ut mens den leses, teller også.
diag-help-vbus = Bra 4,75–5,50 V; oransje 4,40–4,75 V (for lav) eller opp til 5,70 V; rød utover det. Ingen farge under 0,8 V: ingenting forsyner den.
diag-help-cc = Rød over 2,3 V (5 V på CC: kabel eller kontakt). Oransje under 0,2 V med 5 V til stede: en kabel uten Rp.
diag-help-3v3 = Bra 3,135–3,465 V (±5 %), mot en referanse på ±3,3 %; oransje opp til 3,0 / 3,6 V.
diag-help-budget = Oransje når de tilkoblede enhetene kan trekke mer enn kilden gir: bruk PWR-porten.
diag-help-faults = Overstrømsutkoblinger i strømbryteren til porten.
diag-help-at-temp = Oransje fra 85 °C, rød fra 105 °C. Opptil 20 °C avvik fra brikke til brikke.
diag-help-at-fw = Oransje når den avviker fra RP2354B-ens: programmeres på nytt, eller programmeringen mislyktes.
diag-help-bt-module = Oransje mens KVM-en programmerer den (etter en oppdatering, 10 til 20 s). Rød når den ikke kjører fastvaren sin ved denne oppstarten (prøves igjen neste gang den slås på), eller etter 5 mislykkede forsøk på rad: da prøver KVM-en igjen bare hvis knappen holdes inne ved påslag, eller med Fastvare → Prøv igjen.
diag-help-bt-fw = Modulen kjører fastvaren KVM-en har med seg: en oppdatering av KVM-en oppdaterer den også.
diag-help-bt-link-errors = Mislykkede utvekslinger med modulen siden oppstart, programmeringen medregnet.
diag-help-bt-hosts = Sammenkoblede verter som er tilkoblet nå og kan ta imot inndata.

## Innstillinger på kommandolinjen

parse-colour = farge '{ $value }': bruk #rrggbb
parse-hotkey = hurtigtast '{ $value }': { $error } (prøv double:scrolllock eller ctrl+alt+1)
parse-host-name = navn '{ $value }': høyst { $max } byte (tomt: Vert n)
parse-profile-name = navn '{ $value }': 1 til { $max } byte
parse-mode = modus '{ $value }': { $modes }
parse-profile-modes = '{ $value }': tre moduser for tastatur, mus og annet, f.eks. auto,auto,passthrough
parse-number = '{ $value }': et tall opp til { $max }
parse-one-of = '{ $value }': bruk { $choices }
parse-unknown-setting = ukjent innstilling

## Vinduet: sidepanel og topp

window-title-sim = { -product } (simulert)
simulated-kvm = simulert KVM
page-overview = Oversikt
page-ports = Porter
page-displays = Skjermer
page-keyboard = Tastatur
page-settings = Innstillinger
page-firmware = Fastvare
page-wireless = { -wireless }
light-mode = Lyst tema
dark-mode = Mørkt tema
no-kvm-connected = Ingen KVM tilkoblet
leds = Lys
active-pc = Aktiv PC
profile = Profil
connected-to = Tilkoblet · { $device }
reading = Leser…
not-connected = Ikke tilkoblet

## Oversikt

chip-active = AKTIV
chip-this-computer = denne datamaskinen
pc-vbus = VBUS { $volts } V · gir { $offers }
showing = Vises
switch-here = Bytt hit
front-ports = Porter foran
unnamed-device = (enhet uten navn)
host-has-input = { $host } får tastaturet og musa { -wireless(case: "adverb") }. Porter med gjennomkobling blir hos { $pc }.
power = Strøm
power-left = Fra { $source }, omtrent { $milliamps } mA igjen til portene foran
budget-low = De tilkoblede enhetene kan trenge mer: bruk PWR-porten.
selftest-line = Selvtest { $result }
recent = Nylig

## Porter

port-keyboard = Tastaturport
port-mouse = Museport
port-other = Porten Annet
port-short-keyboard = Tastatur
port-short-mouse = Mus
port-short-other = Annet
redetect = Gjenkjenn på nytt
mode = Modus
no-device = Ingen enhet
port-module-summary = { -wireless(case: "prefix-title") }modul, ikke for PC-ene
port-module-note = Denne porten har KVM-ens { -wireless(case: "prefix") }modul, et ESP32-C3-kort: den betjener de { -wireless(case: "plural") } vertene og er ikke koblet til noen PC. Koble fra kortet for å bruke porten til en enhet igjen. For å nå et eget ESP32-kort fra en PC, sett porten til en gjennomkoblingsmodus: et ESP32-kort går da som det er til PC-en.
running-mode = Kjører som { $mode }
up-to-ma = opptil { $milliamps } mA
over-current-faults = { $count ->
    [one] { $count } overstrømsfeil
   *[other] { $count } overstrømsfeil
}
profiles = Profiler
profiles-note = Hver profil er et sett portmoduser, f.eks. emulerte enheter for arbeid og gjennomkobling for spill. Et langt trykk på KVM-ens knapp velger den neste (siden Tastatur). Portene nedenfor følger den aktive profilen.
name = Navn
profiles-in-use = Profiler i bruk:
remember-profile = Start i profilen som sist var aktiv (ellers i profil 0, og å velge en profil skriver ingenting til KVM-ens flashminne)
config-locked = Konfigurasjon fra programvare er slått av på KVM-en.
config-locked-how = Slik slår du det på igjen: Innstillinger → Styring fra programvare, og trykk deretter på knappen på KVM-en.

## Tastatur

hotkeys = Hurtigtaster
hotkey-toggle = Bytt PC
hotkey-pc1 = Velg PC1
hotkey-pc2 = Velg PC2
hotkey-next = Neste mål
cant-learn-firmware = Fastvaren til denne KVM-en kan ikke lære hurtigtaster: skriv den inn i stedet
cant-learn-not-active = Bare den aktive PC-en kan lære en hurtigtast: bytt KVM-en til denne PC-en først
cant-learn-no-keyboard = Ingen tastatur som KVM-en leser er koblet til
learning-another = En annen hurtigtast læres nå
learned-with-warning = { $warning } Bruk tar den i bruk likevel.
learned = Lært fra tastaturet. Bruk tar den i bruk.
not-learned = Ikke lært: { $why }.
kvm-no-answer = KVM-en svarte ikke.
learn-hint-sim = Eller trykk Lær, og deretter hurtigtasten i dette vinduet: den spiller tastaturet til den simulerte KVM-en.
learn-hint = Eller trykk Lær, og deretter hurtigtasten på KVM-ens tastatur.
press-the-hotkey = Trykk hurtigtasten… { $seconds } s
apply = Bruk
cancel = Avbryt
learn = Lær
learn-hover = Trykk hurtigtasten på KVM-ens tastatur i stedet for å skrive den
passthrough-keyboard-note = Et tastatur med gjennomkobling går rett til PC-en: KVM-en ser det ikke, så ingen hurtigtast virker på det, og ingen læres fra det.
double-tap-window = ms vindu for dobbelttrykk
kvm-key = Knappen på KVM-en
tap = Trykk
long-press = Langt trykk
press-tap = trykk
press-long-from = langt trykk fra { $seconds } s
held-for-long-press = Holdt for et langt trykk:
kvm-key-note = Uansett hva som er valgt her, bekrefter et trykk en fastvareoppdatering som venter, og i hvilemodus vekker det første trykket bare KVM-en.
keymap-vial = Tastaturoppsett (Vial)
keymap-note = Rediger oppsettet med Vial-appen mens grensesnittet vises. Oppsettet virker også med grensesnittet slått av. Å endre grensesnittet får begge PC-ene til å gjenkjenne KVM-en på nytt (omtrent 1 s).
vial-interface = Vial-grensesnitt:
vial-off = Av
vial-auto = Automatisk (når ingen speilet enhet trenger endepunktet)
vial-always = Alltid
apply-keymap = Bruk oppsettet på tastaturer KVM-en normaliserer

## Innstillinger

this-computer = Denne datamaskinen
autostart = Start med systemet, i systemstatusfeltet
could-not-change = Kunne ikke endre det: { $error }
appearance = Utseende:
theme-system = System
theme-dark = Mørkt
theme-light = Lyst
language = Språk:
language-system = System
brightness = lysstyrke
pc-colour = Farge { $pc }
pc-side = PC-siden
polling = Avspørring:
polling-125us = 125 µs (lavest forsinkelse)
polling-1ms = 1 ms (for kresne BIOS-er)
power-cycle = Gjør enheter med gjennomkobling strømløse et øyeblikk når de bytter PC
standby = Når PC-ene er av eller sover
standby-note = Når ingen PC har brukt KVM-en på 10 s, slukker lysene og portene foran slås av. En PC som slås på, eller et trykk på KVM-ens knapp, vekker den.
auto-select = Følg PC-en som er på: når den aktive PC-en slås av, blir den andre aktiv
standby-wake = Hold tastatur og mus strømforsynt mens en PC sover, så et tastetrykk vekker den
standby-glow = La knappen gløde i hvilemodus (ellers er alle lys av)
save-now = Lagre nå
factory-settings = Fabrikkinnstillinger
unsaved-changes = Ulagrede endringer skrives om noen sekunder.
software-control = Styring fra programvare
software-control-note = Begge PC-ene kan slå disse av. Å slå dem på igjen krever knappen på KVM-en: et trykk innen { $seconds } s etter at du ba om det her, eller knappen holdt inne mens KVM-en slås på.
allow-switching = Tillat bytte fra programvare
allow-config = Tillat konfigurasjon fra programvare
not-turned-on = Ikke slått på: knappen på KVM-en ble ikke trykket i tide.
press-key-to-confirm = Trykk på knappen på KVM-en for å bekrefte ({ $seconds } s igjen). Lyset pulserer hvitt.
confirm-lock-switching = Slå av bytte fra programvare?
confirm-lock-config = Slå av konfigurasjon fra programvare?
turn-off = Slå av

## Fastvare

versions = Versjoner
wireless-module = { -wireless(case: "prefix-title") }modul
hardware = Maskinvare
serial = Serienummer
uptime = Oppetid
gave-up-note-chip = KVM-en sluttet å prøve etter 5 mislykkede forsøk på rad, så den ikke programmerer brikken på nytt hver gang den slås på. Den prøver igjen med ny fastvare, når knappen holdes inne ved påslag, eller nå:
try-again = Prøv igjen (KVM-en starter på nytt)
update = Oppdatering
image-file = Bildefil:
install = Installer
press-key-to-install = Trykk på knappen på KVM-en for å installere oppdateringen.
installed = Installert; KVM-en starter på nytt.
fw-file-title = Fastvarefil
fw-file-hint = mappen til en utgivelse, kvm-rp2354.bin eller en .uf2
browse = Bla gjennom…
fw-shipped = Medfølgende fastvare
fw-shipped-note = følger med denne appen
fw-file-is = Fastvare { $version } for maskinvare { $major }.x
fw-same = KVM-en har allerede denne versjonen.
fw-newer = Nyere enn KVM-ens { $kvm }.
fw-older = Eldre enn KVM-ens { $kvm }: å installere den går tilbake.
fw-no-file = Ingen fastvare valgt: velg en fil eller mappen til en utgivelse.
update-over-pc-note = Gjennom denne datamaskinens port, til KVM-en som kjører. Den nye fastvaren starter på prøve: kommer den ikke opp, går KVM-en tilbake til den gamle.
pick-firmware = Velg KVM-ens fastvare
firmware-files = KVM-fastvare
boot-title = I oppstartslasteren (PWR/SERVICE-porten)
boot-kvm = { -product } med fastvare { $version }; en oppdatering går i partisjon { $slot }.
boot-kvm-empty = { -product } uten fastvare; den går i partisjon { $slot }.
boot-new = Et nytt kort, ennå tomt. Å sette det opp skriver partisjonstabellen og fastvaren.
boot-new-unpartitioned = Fastvare { $version }, men ingen partisjonstabell (programmert med en feilsøkingsadapter). Å sette det opp skriver begge.
boot-other = Ikke en { -product }: en RP2350 med en annen partisjonstabell eller annen fastvare. Ingenting installeres på den.
boot-unknown-note = Uten det kan ikke denne appen skille en { -product } fra et nytt kort: installer bare på en { -product } som har fastvare, og sett bare opp et nytt kort.
boot-drive = Stasjon: { $path }
boot-no-drive-note = Stasjonen er ikke montert: installering monterer den, eller åpne den i filbehandleren først.
boot-set-up = Sett opp
boot-writes = Skriver { $file }: fastvare { $version } for maskinvare { $major }.x
boot-writing = skriver
boot-restart = Start på nytt uten å installere
boot-restart-hover = Forlater oppstartslasteren: kortet starter fastvaren det har.
boot-installed = Installert: kortet starter på nytt med den nye fastvaren. Koble det til PC-ene for å bruke det.
boot-none-title = Installer gjennom PWR/SERVICE-porten
boot-none-note = For et nytt kort, eller et der fastvaren ikke starter: koble denne datamaskinen til KVM-ens PWR/SERVICE-port. Et nytt kort dukker opp her av seg selv; ethvert annet når knappen i det lille hullet holdes inne mens det kobles til.
boot-found-hint = En { -product } venter i oppstartslasteren på PWR/SERVICE-porten.
open-firmware-page = Fastvaresiden
identify = Identifiser (blink med lysene)
diagnostics = Diagnostikk
measuring = Måler…
diag-bad = { $count } dårlig
diag-warnings = { $count ->
    [one] { $count } advarsel
   *[other] { $count } advarsler
}
diag-all-good = alt i orden
diag-readings = { $count } målinger, for { $seconds } s siden
copy = Kopier
copy-hover = Kopier målingene som tekst
looking-for-kvm = Leter etter KVM-en…
plug-in-kvm = Koble KVM-en til denne datamaskinen.
udev-hint = Installert fra kildekoden? Installer da software/odkvmctl/udev/70-opendyn-kvm.rules (DEVELOPMENT.md).

## Trådløse verter (Bluetooth-funksjonen; navnet er termen -wireless)

host-connecting = kobler til…
host-hover = { -wireless }, { $state }
send = Send
cancel-pairing = Avbryt sammenkobling
host-pairing = En { -wireless(case: "lower") } vert kobles sammen
passkey-text = Verten viser seks sifre: skriv dem på KVM-ens tastatur og trykk Enter (Escape avbryter), eller skriv dem her. Til da skriver KVM-ens tastatur ingen andre steder.
passkey-typed = På KVM-ens tastatur: { $digits }
module-is = Modulen { $state }.
module-programming-note = Etter en fastvareoppdatering programmerer KVM-en modulen med fastvaren den har med seg, noe som tar 10 til 20 s. De { -wireless(case: "plural") } vertene kobler seg til igjen etterpå.
module-failed-note = Den { -wireless(case: "plural") } funksjonen er av til KVM-en slås på igjen, og da prøver den én gang til. De to PC-ene virker som før.
gave-up-note-module = KVM-en sluttet å prøve etter 5 mislykkede forsøk på rad, så den ikke programmerer modulen på nytt hver gang den slås på. Den prøver igjen med ny fastvare, når knappen holdes inne ved påslag, eller nå:
module-on-port = Modulen er et ESP32-C3-kort i { $port }.
module-failed-note-board = Kortet kjører ikke modulens fastvare, så den { -wireless(case: "plural") } funksjonen er av til KVM-en starter på nytt. De to PC-ene virker som før. Prøv igjen nå:
gave-up-note-board = KVM-en sluttet å prøve etter 5 mislykkede forsøk på rad, så den ikke programmerer kortet på nytt hver gang det kobles til. Den prøver igjen når KVM-en starter på nytt, eller nå:
pair-new-host = Koble til en ny vert
pair-note = Gjør KVM-en synlig som «{ -product }» i ett minutt. Koble den deretter til fra PC-en, nettbrettet eller telefonen i { -host-pairing-settings }, som et tastatur. Viser verten seks sifre, skriver du dem på KVM-ens tastatur og trykker Enter.
pair = Koble til
slots-full = Glem en vert først: alle fire plassene er i bruk
discoverable = Synlig · { $seconds } s
stop = Stopp
hosts = Verter
hosts-note = De { -wireless(case: "plural") } vertene får KVM-ens eget tastatur og mus (speiling, identitet og gjennomkobling blir hos PC-ene). «Hold tilkoblet» gjør byttet umiddelbart; «Bare når valgt» passer for telefoner og nettbrett som skjuler skjermtastaturet når et tastatur er tilkoblet.
slot-free = ledig
active = aktiv
forget = Glem
forget-hover = Fjerner sammenkoblingen. For å koble til igjen må du også fjerne «{ -product }» på verten.
link-every = Hvert { $ms } ms
link-may-skip = { $count ->
    [one] kan hoppe over { $count }
   *[other] kan hoppe over { $count }
}
link-signal = signal { $dbm } dBm
policy-stay = Hold tilkoblet
policy-on-select = Bare når valgt
a-host = en { -wireless(case: "lower") } vert

## Systemstatusfeltet

tray-not-connected = { -product }: ikke tilkoblet
tray-host-active = { -product }: { $host } aktiv ({ -wireless(case: "lower") })
tray-pc-active = { -product }: { $pc } aktiv
tray-switch-to = Bytt til { $target }
tray-open = Åpne
tray-quit = Avslutt

## Nylige hendelser

ev-switched = Byttet til { $target } ({ $source })
ev-port-changed = { $port } endret
ev-fault-port = { $port }: { $fault }
ev-fault = Feil: { $fault }
ev-setting-changed = Innstillingen { $name } endret
ev-hotkey-learned = Hurtigtast lært: { $hotkey }
ev-hotkey-not-learned = Lære en hurtigtast: { $why }
ev-pairing-open = Sammenkoblingen er åpen ({ -wireless(case: "lower") })
ev-pairing-closed = Sammenkoblingen er lukket ({ -wireless(case: "lower") })
ev-passkey-wanted = En { -wireless(case: "lower") } vert ber om koden sin
ev-paired = Koblet til: { $host } (plass { $slot }; gi den navn på siden { -wireless })
ev-forgot = Glemte den { -wireless(case: "plural") } verten { $host }
ev-host-connected = { $host } tilkoblet
ev-host-disconnected = { $host } frakoblet
ev-switched-host = Byttet til { $host } ({ -wireless(case: "lower") })
ev-monitor-not-switched = Skjermen ble ikke byttet: { $error }
fault-over-current = overstrøm
fault-enumeration = gjenkjenning mislyktes
fault-link-down = forbindelsen er brutt
fault-power-budget = ikke nok strøm
fault-hub = hubfeil
no-diagnostics = Fastvaren til denne KVM-en har ingen diagnostikk ennå: oppdater den.
kvm-refused = KVM-en avviste: { $why }.

## Skjermer som følger KVM-en (DDC/CI)

profile-n = Profil { $n }
arrangement-n = Oppsett { $n }
display-follow = Følger KVM-en
display-pc1 = Blir på PC1
display-pc2 = Blir på PC2
display-off = Byttes ikke
input-none-pc = — byttes ikke
input-none-host = — blir hos PC-en
display-switched = { $monitor } byttet til { $input }
displays-title = Skjerminngangene følger KVM-en
displays-note = Når KVM-en bytter PC (eller til en { -wireless(case: "lower") } vert) eller profil, bytter denne datamaskinen skjermene nedenfor (DDC/CI): i hver profil kan en skjerm følge KVM-en, bli på én PC eller stå i fred. KVM-en melder hver endring til begge PC-ene, så sett helst opp hver skjerm på bare én PC.
displays-two-pcs = De to PC-ene deler ikke disse innstillingene: en skjerm som er satt opp på begge, får hver kommando to ganger, og følger ingen av oppsettene hvis de er ulike. Én PC holder hvis skjermen også lytter på en inngang den ikke viser: «Vis PC1» / «Vis PC2» bort fra denne PC-en og tilbake viser det. Kommer den ikke tilbake, setter du den opp likt på begge PC-ene.
displays-ddc-on = DDC/CI må være slått på i skjermens egen meny.
look-again = Let etter skjermer igjen
asking-monitors = Spør skjermene…
this-computer-is = Denne datamaskinen er { $pc }
kvm-not-connected = KVM ikke tilkoblet
no-monitor = Ingen skjerm svarte. Sjekk at DDC/CI er slått på i skjermens meny.
saved-not-connected = Lagret, ikke tilkoblet nå
input-other = annen
input-other-hover = En inngang skjermen ikke lister opp, heksadesimalt (f.eks. 0x0F), deretter Enter
use-current = Bruk gjeldende
use-current-hover = Inngangen denne skjermen viser nå
show-target = Vis { $target }
monitor-shows = viser { $input }
not-answering = svarer ikke
target-input = Inngang for { $target }
displays-hosts-note = De { -wireless(case: "plural") } vertene er valgfrie: mens én av dem har tastaturet og musa, viser en skjerm som følger KVM-en den hvis den har en inngang her, og blir ellers hos PC-en.
in-each-profile = I hver profil:
input-missing = En profil trenger en inngang som ikke er valgt ovenfor.
could-not-save = Kunne ikke lagre innstillingene til denne datamaskinen: { $error }
input-unknown = Inngang { $value }
ddc-no-answer = ikke noe svar (er DDC/CI slått på i skjermens meny?)
ddc-garbled = forvrengt svar
ddc-unexpected = uventet svar
ddc-unsupported-code = skjermen støtter det ikke
ddc-not-connected = { $id }: ikke tilkoblet
ddc-unsupported-os = DDC/CI støttes bare på Windows og Linux
ddc-no-access = { $path }: ingen tilgang. Installer pakken, eller udev-regelen fra README, og logg inn på nytt.
ddc-no-i2c-dev = { $path }: mangler, modulen i2c-dev er ikke lastet. Installer pakken, eller last den slik README beskriver.

## Kommunikasjon med KVM-en

error-timeout = KVM-en svarte ikke
error-device = KVM-en avviste: { $why }
error-protocol = protokollfeil: { $error }
protocol-unexpected = uventet svar { $answer }
protocol-update-ended = oppdateringen endte i tilstanden { $state }
error-not-confirmed = knappen på KVM-en ble ikke trykket i tide: ingenting ble endret
host-n = Vert { $n }
hid-open-linux = { $error } (installer software/odkvmctl/udev/70-opendyn-kvm.rules for tilgang uten root)
no-kvm-found = ingen { -product } funnet

## Fastvarefiler

img-damaged-uf2 = skadet UF2-fil
img-uf2-partition-table = denne UF2-filen inneholder også partisjonstabellen (kvm-first.uf2); bruk kvm-rp2354.uf2 eller kvm-rp2354.bin
img-uf2-other-chip = UF2 for en annen brikke (familie { $family })
img-uf2-out-of-range = UF2-blokk ved { $address } er utenfor området
img-too-big = { $kb } KB er mer enn en fastvarepartisjon rommer ({ $max } KB)
img-not-rp2354b = ikke RP2354B-fastvare (ingen RP2350-bildedefinisjon)
img-wrong-kind = RP2350-bilde av feil type (flagg { $flags })
img-no-fw-id = ikke { -product }-fastvare (ingen fastvare-ID)
img-other-hardware = fastvare { $version } er for maskinvare { $major }.x, denne KVM-en er maskinvare { $hw }
img-not-first = ikke en UF2 for førstegangsinstallasjon (kvm-first.uf2: partisjonstabell og fastvare)
fw-read = { $file }: { $error }
fw-not-in-folder = ingen { $name } i { $folder }
boot-no-picoboot = oppstarts-ROM-en viser ikke noe PICOBOOT-grensesnitt
boot-no-permission = ingen tillatelse til RP2350-ens oppstarts-ROM (USB 2e8a:000f): installer udev-regelen (software/odkvmctl/udev/70-opendyn-kvm.rules)
boot-no-access = kan ikke åpne RP2350-ens oppstarts-ROM: { $error }
boot-usb = USB: { $error }
boot-rom-ignored = oppstarts-ROM-en ignorerte filen (ikke for dette kortets partisjoner)
boot-rom-locked = et annet program holder oppstarts-ROM-en (picotool?)
boot-rom-bad-address = oppstarts-ROM-en stoppet: en adresse utenfor flashminnet
boot-rom-write-error = oppstarts-ROM-en kunne ikke skrive flashminnet
boot-rom-reboot-failure = oppstarts-ROM-en skrev fastvaren, men kunne ikke starte den
boot-no-drive = oppstarts-ROM-ens stasjon (RP2350) er ikke montert: åpne den i filbehandleren, og installer igjen
boot-write = skriving til oppstarts-ROM-ens stasjon: { $error }
boot-no-restart = kortet startet ikke på nytt etter kopieringen (ingenting på 20 s)

## odkvmctl: hjelp

cli-about = Styr { -product }-svitsjen
cli-sim = Snakk med en innebygd simulert KVM i stedet for den ekte
cli-status = Vis KVM-ens tilstand (standard)
cli-diag = Mål KVM-ens klokker, spenninger, temperaturer og USB-forbindelser, hver vurdert som bra, advarsel eller dårlig; avslutter med 1 hvis noe er dårlig
cli-list = List opp KVM-styringsgrensesnittene denne datamaskinen ser
cli-watch = Skriv ut hendelser (bytter, portendringer, feil) etter hvert som de skjer
cli-switch = Bytt tastaturet, musa og den andre enheten til en PC, eller (med den { -wireless(case: "plural") } modulen) til en { -wireless(case: "lower") } vert
cli-wireless = De { -wireless(case: "plural") } vertene (KVM-er med modulen): list dem opp (standard), koble til en ny, glem, gi navn, eller bestem hva som skjer med en vert mens et annet mål er aktivt
cli-port = Vis en port foran, sett modusen, eller gjenkjenn enheten på nytt
cli-profile = List opp profilene, gjør én aktiv (profile next, profile 1, profile gaming), eller endre én (profile 1 name gaming, profile 1 mouse passthrough)
cli-profile-which = Nummeret eller navnet på en profil, eller next
cli-profile-setting = name, arrangement, eller en port: keyboard, mouse, other
cli-hotkey = Vis eller sett en hurtigtast: double:scrolllock, double:rctrl:noswallow, ctrl+alt+1, none; eller lær den (learn) fra de neste tastene som trykkes på KVM-ens tastatur
cli-config = Les og endre innstillinger
cli-identify = Blink med KVM-ens lys
cli-update = Installer RP2354B-fastvare: på et kort som venter i oppstartslasteren på PWR/SERVICE-porten hvis det finnes ett, ellers gjennom en PC-port (ber om et trykk på KVM-ens knapp)
cli-reboot = Start KVM-en på nytt, eller sett en brikke i oppstartslasteren
cli-display = Skjermer denne datamaskinen kan bytte inngang på via DDC/CI (ingen KVM nødvendig)
cli-display-list = Skjermene, inngangen hver viser og inngangene den tilbyr (standard; tar noen sekunder)
cli-display-input = Bytt en skjerm til en inngang, f.eks. display input XYZ-1A2B-1234 0x11 (ID-er fra display list)
cli-hosts-list = De fire vertsplassene: navn, tilstand, oppførsel
cli-hosts-pair = Gjør KVM-en synlig i 60 s som «{ -product }»; koble den deretter til på PC-en, nettbrettet eller telefonen. Viser verten seks sifre, skriv dem på KVM-ens tastatur og trykk Enter (eller bruk passkey)
cli-hosts-pair-stop = stop lukker sammenkoblingen tidligere
cli-hosts-passkey = De seks sifrene en vert som kobles sammen viser, når KVM-ens tastatur ikke er for hånden
cli-hosts-forget = Glem en verts sammenkobling: plass 1-4, eller all
cli-hosts-name = Gi en vertsplass navn (tomt: «Vert n»)
cli-hosts-policy = stay: verten forblir tilkoblet, og å bytte til den er umiddelbart. on-select: tilkoblet bare mens den er valgt (for telefoner og nettbrett som skjuler skjermtastaturet når et tastatur er tilkoblet)
cli-port-redetect = Gjenkjenn enheten på nytt (nødvendig etter å ha byttet enhet på en port med gjennomkobling)
cli-config-get = Vis én innstilling, eller alle
cli-config-set = Endre en innstilling (config get viser navn og gjeldende verdier)
cli-config-save = Skriv innstillingene til KVM-ens flashminne nå (ellers 5 s etter siste endring)
cli-config-reset = Tilbake til fabrikkinnstillingene (programvarelåsene beholdes)
cli-target-next = Neste mål: PC1, PC2, deretter hver tilkoblet { -wireless(case: "lower") } vert
cli-reboot-at32-retry = Gi AT32F405-ene opptil 5 nye programmeringsforsøk, og start deretter på nytt
cli-reboot-wireless-retry = Gi { -wireless(case: "prefix") }modulen opptil 5 nye programmeringsforsøk, og start deretter på nytt

## odkvmctl: utdata

cli-port-auto = automatisk: { $why }
cli-port-set-to = satt til { $mode }: { $why }
cli-status-head = { -product } ({ $device }): fastvare { $firmware }, AT32 { $at1 } / { $at2 }, maskinvare { $hardware }, serienummer { $serial }
cli-module-firmware = { -wireless(case: "prefix-title") }modul: fastvare { $version }
cli-module-state = { -wireless(case: "prefix-title") }modul: { $state }
cli-module-port = { -wireless(case: "prefix-title") }modul: et ESP32-C3-kort i { $port }
cli-active-host = Aktiv: { -wireless(case: "lower") } vert { $slot } «{ $host }» (hjemme-PC { $pc })
cli-active = Aktiv: { $target }
cli-this-computer = denne datamaskinen er { $pc }
cli-profile-line = Profil: { $n } ({ $name })
cli-host-slot = Vert { $slot }
cli-pairing-open = sammenkoblingen er åpen ({ -wireless(case: "lower") }, { $seconds } s igjen)
cli-passkey-waits = en vert som kobles sammen venter på koden sin: skriv den på KVM-ens tastatur, deretter Enter
cli-budget-low = enhetene kan trenge mer
cli-power = Strøm: { $power }
cli-selftest = Selvtest: { $result }
cli-try-again = prøv igjen: { $command }, eller hold KVM-ens knapp inne mens den slås på
cli-ports = Porter:
cli-switching-disabled = bytte fra programvare er slått av
cli-config-disabled = konfigurasjon fra programvare er slått av
cli-locked = Låst på enheten: { $what }
cli-host-ready = tilkoblet, { $ms } ms
cli-profiles-in-use = { $count } av { $max } i bruk: odkvmctl config set profiles <1-{ $max }>
cli-no-profile = ingen profil '{ $which }': et tall fra 0 til { $last } eller et navn
cli-learn-sim = tastaturet til den simulerte KVM-en kan ikke trykkes herfra: prøv Lær i `opendyn-kvm --sim`
cli-learn-press = Trykk den nye hurtigtasten på KVM-ens tastatur innen { $seconds } s. Tastene når ingen PC imens.
cli-learn-unsupported = fastvaren til denne KVM-en kan ikke lære hurtigtaster: oppgi hurtigtasten som tekst, f.eks. ctrl+alt+1
cli-nothing-changed-why = { $why }: ingenting ble endret
cli-use-it-for = Bruke den for { $name }? [j/N]
cli-turn-it-off = Slå det av? [j/N]
## Ord, i tillegg til y og yes, som svarer ja på et [j/N]-spørsmål, adskilt med komma.
cli-yes-words = j, ja
cli-readings = { $count } målinger: { $verdict }
cli-ev-back-on-pc = inndata tilbake på en PC
cli-ev-host-active = byttet til { -wireless(case: "lower") } vert { $slot }
cli-ev-pairing-changed = sammenkoblingen endret ({ -wireless(case: "lower") })
cli-ev-host-changed = { -wireless(case: "lower") } vert { $slot } endret
cli-no-monitors = ingen skjermer funnet
cli-inputs = innganger: { $inputs }
cli-input-number = inngang '{ $value }': et tall som 0x11 eller 17
cli-no-interface = ingen { -product }-styringsgrensesnitt funnet
cli-connecting = kobler til KVM-en
cli-watching = følger med på { $device } (Ctrl+C for å stoppe)
cli-no-module = denne KVM-en har ingen { -wireless(case: "prefix") }modul
cli-no-host-in-slot = ingen vert er koblet til på den plassen
cli-no-module-boards = denne KVM-en har ingen { -wireless(case: "prefix") }modul (kort fra 0.16.0 kan ha en)
cli-pair-usage = '{ $value }': bruk `wireless pair` eller `wireless pair stop`
cli-pairing-opened = Sammenkoblingen er åpen i { $seconds } s: koble til «{ -product }» på PC-en, nettbrettet eller telefonen.
cli-pairing-digits = Viser den seks sifre, skriv dem på KVM-ens tastatur og trykk Enter.
cli-six-digits = de seks sifrene verten viser
cli-no-passkey-wanted = ingen vert venter på en kode
cli-slot-or-all = plass 1-4 eller all
cli-slot = plass 1-4
cli-no-profiles = denne fastvaren har ingen profiler
cli-profile-not-in-use = profil { $n } er ikke i bruk: odkvmctl config set profiles { $count }
cli-missing-value = profil { $setting }: verdien mangler
cli-nothing-changed = Ingenting ble endret.
cli-unknown-setting = ukjent innstilling '{ $name }'
cli-press-key-within = Trykk på knappen på KVM-en innen { $seconds } s for å bekrefte…
cli-saved = lagret
cli-factory-restored = fabrikkinnstillingene er gjenopprettet
cli-reading = leser { $file }
cli-firmware-for = fastvare { $version } for maskinvare { $major }.x
cli-update-file = Fastvaren: mappen til en utgivelse, kvm-rp2354.bin eller en .uf2 (standard: fastvaren som følger med dette programmet)
cli-boot-no-drive = stasjonen er ikke montert
cli-no-firmware = ingen fastvare oppgitt, og ingen følger med dette programmet
cli-no-kvm-anywhere = ingen { -product } på en PC-port, og ingen i oppstartslasteren på PWR/SERVICE-porten
cli-boot-several = flere kort i oppstartslasteren: koble til ett om gangen
cli-boot-unknown = Uten det kan ikke odkvmctl skille en KVM fra et nytt kort. Kopier selv kvm-rp2354.uf2 (en KVM med fastvare) eller kvm-first.uf2 (et nytt kort) til RP2350-stasjonen.

## Skjermhopp (funksjonens navn er termen -hop)

page-screens = { -hop }
switch-source-edge = pekeren krysset en skjermkant
hop-line = { -hop }: { $state }
hop-state-off = av i denne profilen
hop-state-active = på
hop-state-no-absolute = trenger den absolutte pekeren
hop-state-not-normalized = trenger museporten i modusen Normalisert
hop-state-no-screens = den aktive datamaskinen har ingen skjerm i profilens oppsett
hop-state-boot-protocol = av mens PC-en er i fastvareoppsettet sitt
hop-state-wireless = av mens en { -wireless(case: "lower") } vert har inndataene
accel-flat = Lineær (bare hastighet)
accel-precision = Presisjonskurve, som på Windows
accel-adaptive = Adaptiv, som på Linux
mapping-desktop = Over alle skjermene sine (Linux)
mapping-main-screen = Bare på hovedskjermen sin (Windows)
units-physical = fysiske piksler (Windows, X11)
units-logical = logiske piksler (Wayland)
bt-pointer-absolute = Absolutt (nøyaktig)
bt-pointer-relative = Relativ (for systemer uten absolutt peker)
parse-screen = '{ $value }': skriv en skjerm som pc1 2560x1440 150% at 1920,0 desk 0,0 (pc1, pc2, eller host1–host4 for en { -wireless(case: "lower") } vert; skalering og posisjoner kan utelates; legg til no-hop for å holde pekeren unna den, fixed for å beholde plassen på datamaskinen), eller off
parse-range = '{ $value }': et tall fra { $min } til { $max }
screens-none-found = fant ingen skjermer
screens-xrandr-missing = kan ikke kjøre xrandr for å liste skjermene ({ $error })
screens-xrandr-failed = xrandr klarte ikke å liste skjermene: { $error }
screens-unsupported-os = å finne skjermene støttes bare på Windows og Linux
profile-hop = Hopp
profile-hop-hover = { -hop } i denne profilen (egen side)
hop-firmware-too-old = Fastvaren på denne KVM-en har ikke { -hop(case: "lower") }: oppdater den.
hop-what = KVM-en flytter pekeren selv, over skjermene som er satt opp nedenfor. Skyv den over en kant inn på en annen datamaskins skjerm (den andre PC-ens eller en { -wireless(case: "lower") } verts), så bytter KVM-en til den, tastaturet også.
hop-turn-on-absolute = Slå på den absolutte pekeren
hop-mouse-normalized = Kjør museporten normalisert i denne profilen
hop-in-profiles = Oppsett i hver profil
hop-profile-active = { $name } (aktiv)
hop-absolute-pointer = Absolutt peker på KVM-ens mus
hop-absolute-pointer-note = { -hop } trenger den. Å slå den på eller av får begge PC-ene til å koble til KVM-ens enheter på nytt én gang.
hop-arrangement = Oppsett
hop-arrangement-note = Dra skjermene dit de står. Pekeren går over der to skjermer møtes, og hopper over et mellomrom mellom dem; en kant uten noe ved siden av holder pekeren tilbake.
hop-overlap = Noen skjermer overlapper: pekeren kan ikke gå mellom dem. En skjerm koblet til begge PC-ene: slå av { -hop(case: "lower") } for den på PC-en som ikke viser den, og legg den oppå skjermen til den andre PC-en.
hop-add-screen = Legg til en skjerm for { $pc }
hop-detect = Finn skjermene til { $pc }
hop-detect-hover = Denne datamaskinens skjermer, med oppløsning, skalering og plassering i skjerminnstillingene
hop-remove-screen = Fjern
hop-draft = Ikke tatt i bruk ennå: KVM-en beholder oppsettet sitt til du tar endringene i bruk.
hop-applying = Tatt i bruk: venter på at KVM-en bekrefter. Avviser den, blir endringene her så du kan prøve igjen.
hop-apply = Bruk
hop-discard = Forkast endringene
hop-mapping-of = { $pc } plasserer pekeren
hop-units-of = Enhetene i skjerminnstillingene til { $pc }
hop-arrangement-none = ingen (av)
hop-arrangement-in-use = { $name } (i bruk)
hop-arrangement-name = Navn
hop-bt-pointer-of = Pekeren til { $host }
hop-bt-pointer-note = En { -wireless(case: "lower") } vert får en absolutt peker som PC-ene: markøren havner nøyaktig der KVM-ens peker er. Et system som ikke tar en absolutt peker (noen nettbrett og telefoner), trenger «Relativ»: KVM-en skyver markøren inn i øvre venstre hjørne når pekeren kommer inn på verten, og flytter den derfra. Verten akselererer den bevegelsen, så markøren glir bort fra KVM-ens peker til den skyves mot en kant.
hop-mapping-note = Windows legger en absolutt peker bare på hovedskjermen. KVM-en når de andre skjermene ved å flytte markøren dit slik en mus gjør, og Windows akselererer det: slå av «Forbedre pekerpresisjon» på en slik PC for nøyaktige kanter, eller la { -product } kjøre på den.
hop-mapping-windows = Denne datamaskinen kjører Windows, som legger den absolutte pekeren bare på hovedskjermen: over alle skjermene havner pekeren feil sted, og de andre skjermene kan ikke nås. Velg «Bare på hovedskjermen sin (Windows)».
hop-no-screens = Ingen skjermer ennå: legg til én for hver datamaskin.
hop-main-screen = hovedskjerm
hop-screen-hover = Klikk for å endre (klikk igjen for en skjerm under), dra for å flytte
hop-screen-pc = Datamaskin
hop-screen-resolution = Oppløsning
hop-screen-scale = Skalering
hop-screen-desk = Plassering på datamaskinen
hop-screen-at = Plassering her
hop-screen-used = { -hop } går inn på den
hop-screen-used-note = Av for en skjerm koblet til begge PC-ene som viser den andre: legg den oppå skjermen til den PC-en. PC-en regner den med likevel, så pekeren havner på riktig sted på de andre skjermene.
hop-screen-off-short = uten { -hop(case: "lower") }
hop-screen-desk-note = Hvor skjermen er i skjerminnstillingene til datamaskinen sin, i den datamaskinens piksler (logiske på et Wayland-skrivebord). På Windows er hovedskjermen på 0, 0.
hop-detected = { $count ->
    [one] { $pc } har nå 1 skjerm, slik denne datamaskinen har.
   *[other] { $pc } har nå { $count } skjermer, slik denne datamaskinen har.
} { $pc } plasserer pekeren: { $mapping }.
hop-pointer = Peker
hop-accel = Akselerasjon
hop-speed = Hastighet
hop-resistance = Kantmotstand
hop-corner = Hjørner
hop-pointer-note = Datamaskinene akselererer ikke en absolutt peker, så KVM-en gjør det. Kantmotstand er hvor langt pekeren må skyves mot en kant før den går over til en annen datamaskin; hjørner er hvor langt fra hver ende av kanten den aldri går over, så hjørnene er lette å treffe. Når du drar noe, går den aldri over.
cli-screens = Skjermhopp: oppsettene av datamaskinenes skjermer og hvordan pekeren beveger seg
cli-screens-list = Vis oppsettene og pekerinnstillingene
cli-screens-detect = Legg denne datamaskinens skjermer inn i oppsettet
cli-screens-which-pc = KVM-en sier ikke hvilken PC denne datamaskinen er (simulatoren eller serviceporten): still inn skjermene med odkvmctl config set arrangement0-screen0 … i stedet
cli-hop-mapping = { $pc } plasserer pekeren: { $mapping }
cli-hop-units = Skjerminnstillingene til { $pc } teller i { $units }
cli-hop-pointer = Peker: { $accel }, hastighet { $speed }, kantmotstand { $resistance } px, hjørner { $corner } px
cli-screens-arrangement = Oppsettet, som nummer (0–3) eller navn; standard er den aktive profilens
cli-no-arrangement = ikke noe oppsett '{ $which }': et tall fra 0 til 3 eller et navn
cli-arrangement-unused = i ingen profil
cli-arrangement-used = i { $profiles }
cli-hop-bt-pointer = Pekeren til { $host }: { $pointer }
cli-profile-no-hop = ikke { -hop(case: "lower") }
cli-profile-hop = { -hop(case: "lower") }: { $name }
hop-out-of-date = Skjerminnstillingene på denne datamaskinen avviker fra KVM-ens kopi av skjermene, så pekeren havner på feil sted her: finn skjermene på nytt.
cli-hop-out-of-date = { hop-out-of-date } (odkvmctl screens detect)
