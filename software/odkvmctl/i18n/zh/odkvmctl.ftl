# OpenDynKVM：odkvmctl 与 opendyn-kvm，简体中文。此处缺少的消息以英文显示。

## 品牌。产品和功能名称是术语（term）：在此修改，所有消息随之改变。
## -wireless 即蓝牙功能。产品在蓝牙技术联盟（Bluetooth SIG）登记之前称为“无线”；登记之后改为“蓝牙”。
## 中文没有大小写之分，所以 case 参数在这里不起作用。消息 ID 不能与术语同名（Fluent 只保留其中一个）。

-product = OpenDynKVM
-wireless = 无线
## 手机或电脑配对键盘的地方：主机自己的菜单，沿用其原名。
-host-pairing-settings = 蓝牙设置
## -hop 即跨屏：KVM 让指针跨越两台电脑的屏幕移动，并在屏幕边缘切换电脑。
-hop = 跨屏

product-name = { -product }

## 常用词

yes = 是
no = 否
on = 开
off = 关

## KVM 回复的错误

err-unknown = 未知请求
err-malformed = 请求格式错误
err-locked = 已在 KVM 上关闭（用 `odkvmctl config set software-config on` 并按一下 KVM 上的按键重新开启，或在 KVM 通电时按住按键）
err-busy = 忙，请重试
err-unsupported = 此固件不支持
err-invalid-value = 无效的值
err-needs-key-press = 请按 KVM 上的按键确认
err-no-device = 该端口上没有设备
err-not-active = 只有当前电脑可以这样做：请先将 KVM 切换到这台电脑

## 学习热键

learn-learned = 已学习
learn-timed-out = 未及时按下可组成热键的按键
learn-cancelled = 已停止（KVM 切换了电脑，或学习被取消）
hotkey-warn-twin = 它也是“{ $name }”的热键：按一下会同时执行两者。
hotkey-warn-alone = 单独按 { $key } 就会切换，电脑将不再收到它。

## 关闭软件控制

lock-warn-switching = 两台电脑都将无法通过软件切换 KVM：odkvmctl、本应用及其托盘图标都不行。KVM 上的按键和键盘热键仍然有效。
lock-warn-config = 两台电脑都将无法通过软件更改任何设置：端口模式、方案、热键、固件更新和重启都不行，Vial 变为只读。状态仍然可见。
lock-warn-back-on = 重新开启需要 KVM 上的按键：在请求后 { $seconds } 秒内按一下，或在 KVM 通电时按住按键。

## 与电脑的链路

link-unpowered = 关机
link-detached = 未连接
link-enumerating = 正在连接
link-connected = 已连接
link-suspended = 睡眠
link-down = 链路断开

## 前置端口

port-state-empty = 空
port-state-probing = 正在检测
port-state-hosted = 活动
port-state-passthrough = 直通
port-state-unpowered = 未供电
port-state-fault = 故障
port-state-unsupported = 不支持
port-state-module = { -wireless }模块

mode-auto = 自动
mode-normalized = 标准化
mode-mirrored = 镜像
mode-mirrored-identity = 镜像 + 身份
mode-passthrough = 直通（跟随当前电脑）
mode-passthrough-pc1 = 直通，固定到 PC1
mode-passthrough-pc2 = 直通，固定到 PC2
mode-short-auto = 自动
mode-short-normalized = 标准化
mode-short-mirrored = 镜像
mode-short-identity = 身份
mode-short-passthrough = 直通
mode-short-pinned-pc1 = 固定 PC1
mode-short-pinned-pc2 = 固定 PC2

reason-not-auto = 由你设置
reason-fits-normalized = 所有输入都适合标准化接口
reason-extra-inputs = 有标准化接口无法承载的输入
reason-other-port-hid = 其他端口上的 HID 设备
reason-non-hid = 有非 HID 接口
reason-xinput = XInput 手柄
reason-endpoint-budget = 剩余端点不足以镜像它
reason-identity-taken = 另一个端口已在使用身份

speed-low = 低速
speed-full = 全速
speed-high = 高速

class-keyboard = 键盘
class-mouse = 鼠标
class-consumer = 媒体键
class-gamepad = 手柄
class-xinput = XInput
class-vendor-hid = 厂商输入
class-vendor-channel = 厂商命令
class-other-hid = 其他 HID
class-audio = 音频
class-video = 视频
class-storage = 存储
class-hub = 集线器

## KVM 上的按键

key-action-none = 无
key-action-toggle-pc = 切换到另一台电脑
key-action-next-profile = 下一个方案
key-action-other-port = 轮换其他端口的模式
key-action-next-target = 下一个目标（先电脑，再{ -wireless(case: "lower") }主机）

## 电源

adv-none = 无
adv-default = USB 默认
adv-1a5 = 1.5 A
adv-3a = 3 A
power-source-pwr = PWR 端口
power-source-pc = 电脑 USB

## 固件更新

fw-state-idle = 空闲
fw-state-receiving = 正在上传
fw-state-awaiting-key = 请按 KVM 上的按键以安装
fw-state-writing = 正在安装
fw-state-done = 完成
fw-state-failed = 失败

## 无线模块

bt-module-up = 运行中
bt-module-starting = 正在启动
bt-module-programming = 正在更新
bt-module-failed = 未运行其固件：{ -wireless(case: "lower") }关闭，直到下次通电
bt-module-gave-up = 尝试 5 次后放弃：{ -wireless(case: "lower") }关闭
bt-module-absent = 未安装

## 自检

selftest-hub0 = 未找到 Hub-0（前置端口）
selftest-link1 = 与 PC1 侧芯片无连接
selftest-link2 = 与 PC2 侧芯片无连接
selftest-hub1 = PC1 集线器不工作
selftest-hub2 = PC2 集线器不工作
selftest-port-kb = 键盘端口电源开关
selftest-port-ms = 鼠标端口电源开关
selftest-port-ot = 其他端口电源开关
selftest-power = PWR 端口读数不合理
selftest-at32-fw1 = 烧写 PC1 侧芯片失败（下次通电时重试）
selftest-at32-fw2 = 烧写 PC2 侧芯片失败（下次通电时重试）
selftest-at32-gave-up1 = 尝试 5 次后放弃烧写 PC1 侧芯片
selftest-at32-gave-up2 = 尝试 5 次后放弃烧写 PC2 侧芯片
selftest-at32-xtal1 = PC1 侧芯片的晶振未运行（无法向 PC1 提供 USB）
selftest-at32-xtal2 = PC2 侧芯片的晶振未运行（无法向 PC2 提供 USB）
selftest-bt-fw = { -wireless(case: "lower") }模块未运行其固件（下次通电时重试）
selftest-bt-gave-up = 尝试 5 次后放弃烧写{ -wireless(case: "lower") }模块
selftest-running = 运行中
selftest-passed = 通过
selftest-running-so-far = 运行中，目前：{ $failed }
selftest-failed = 失败：{ $failed }

## KVM 切换的原因

switch-source-key = KVM 上的按键
switch-source-hotkey = 热键
switch-source-software = 软件
switch-source-boot = 启动
switch-source-power = 当前电脑已关机

## 诊断

diag-group-system = 系统
diag-group-clocks = 时钟
diag-group-usb = USB
diag-group-power = 电源
diag-group-pc-chips = 电脑侧芯片
diag-group-wireless = { -wireless }
diag-group-other = 其他

diag-uptime = 运行时间
diag-reset-cause = 上次复位
diag-rp-temp = RP2354B 温度
diag-selftest = 自检
diag-dropped = 丢弃的引擎输出
diag-fw-updates = 通过 USB 更新固件
diag-xosc = 晶振 (XOSC)
diag-pll-sys = 系统 PLL
diag-pll-usb = USB PLL
diag-rosc = 环形振荡器
diag-at-clock = AT32F405 #{ $n } 时钟
diag-hub0-speed = Hub-0 链路
diag-hub0-up = Hub-0 已枚举
diag-phy-swap = D+/D− 交换（勘误 E1）
diag-kb-speed = 键盘端口设备
diag-ms-speed = 鼠标端口设备
diag-ot-speed = 其他端口设备
diag-pc-speed = { $pc } USB 速度
diag-pc-link = { $pc } 链路
diag-pc-resets = { $pc } 总线复位
diag-link-errors = AT32F405 #{ $n } SPI 错误
diag-hub0-errors = 未处理的 Hub-0 变化
diag-enum-fails = 前置端口枚举失败
diag-vbus = { $port } VBUS
diag-cc = { $port } CC
diag-at-3v3 = AT32F405 #{ $n } 处的 3.3 V
diag-budget = 前置端口电量预算
diag-kb-faults = 键盘端口故障
diag-ms-faults = 鼠标端口故障
diag-ot-faults = 其他端口故障
diag-at-temp = AT32F405 #{ $n } 温度
diag-at-fw = AT32F405 #{ $n } 固件
diag-bt-module = ESP32-C3 模块
diag-bt-fw = ESP32-C3 固件
diag-bt-link-errors = ESP32-C3 UART 错误
diag-bt-hosts = 已连接的{ -wireless }主机
diag-unknown = 项目 { $id }

reset-software = 软件重启
reset-power-on = 通电
reset-brown-out = 欠压（供电跌落）
reset-glitch = 供电毛刺
reset-run-pin = RUN 引脚
reset-watchdog = 看门狗（更新后也会出现）
reset-debugger = 调试器
reset-other = 原因 { $cause }
hz-stopped = 0 Hz（已停止）
clock-crystal = 12 MHz 晶振
clock-internal = 内部振荡器：无 USB
swap-found = { $state }，运行时发现
fw-updates-on = 开，固件位于分区 { $partition }
fw-updates-no-table = 关：无分区表（由调试器烧写）
fw-updates-read-failed = 关：启动 ROM 未返回分区表（{ $detail }）
fw-updates-other-table = 关：其他分区表（{ $detail }）
fw-updates-other-layout = 关：分区与此固件的不同（{ $detail }）
fw-updates-unknown-partition = 关：无法确定正在运行哪个分区（{ $detail }）
state-other = 状态 { $state }

diag-help-reset-cause = 欠压或供电毛刺后为橙色：5 V 跌落过。有意的重启没有颜色。
diag-help-rp-temp = 70 °C 起为橙色，85 °C 起为红色。
diag-help-selftest = KVM 某部分不工作时为红色；前置端口的电源开关问题为橙色。
diag-help-dropped = 固件无暇处理的工作。不为 0 时为橙色。
diag-help-fw-updates = KVM 是否接受通过电脑端口的固件更新。不接受时为橙色：括号中的代码说明检查读到了什么，值得报告问题。由调试器烧写的板子没有颜色。
diag-help-xosc = 12 MHz 晶振；未运行时为红色。
diag-help-clk-sys = 以晶振为基准计数。偏差达 1 % 或未比 clk_usb 高 10 % 时为红色（RP2350-E12）。
diag-help-clk-usb = 以晶振为基准计数。偏离 48 MHz 超过 0.25 % 时为红色（USB 全速）。
diag-help-clk-adc = 偏离 48 MHz 超过 1 % 时为红色。
diag-help-at-clock = 芯片在没有晶振的情况下运行时为红色：此时它无法向其电脑提供 USB。
diag-help-hub0-speed = 前置端口的集线器。显示为低速时为红色：对于 PHY 的设置，D+ 和 D− 接反了（勘误 E1）。
diag-help-phy-swap = RP2354B 的 USB 是否交换 D+ 和 D−（勘误 E1）：0.8 至 0.12 版电路板为开。
diag-help-pc-speed = 全速时为橙色：125 µs 轮询需要高速。请换一根线缆或端口。
diag-help-pc-link = 电脑侧芯片不应答时为红色；电脑尚未完成设置时为橙色。
diag-help-link-errors = 自启动以来、电脑侧芯片首次应答之后，与它交换失败的次数。
diag-help-hub0-errors = Hub-0 报告的、KVM 的 USB 主机不处理的端口变化；已被清除，以免遮蔽其他端口。最多几个；更多则值得报告问题。
diag-help-enum-fails = KVM 无法读取的前置端口设备，每个都通过关断再接通端口重试几次。读取时拔出设备也计入。
diag-help-vbus = 4.75–5.50 V 为良好；4.40–4.75 V（偏低）或最高 5.70 V 为橙色；超出则为红色。低于 0.8 V 时没有颜色：无供电。
diag-help-cc = 高于 2.3 V 时为红色（CC 上有 5 V：线缆或触点问题）。有 5 V 时低于 0.2 V 为橙色：线缆没有 Rp。
diag-help-3v3 = 3.135–3.465 V（±5 %）为良好，以 ±3.3 % 的基准测得；到 3.0 / 3.6 V 为橙色。
diag-help-budget = 所接设备可能需要超过供电能力时为橙色：请使用 PWR 端口。
diag-help-faults = 端口电源开关的过流跳闸次数。
diag-help-at-temp = 85 °C 起为橙色，105 °C 起为红色。不同芯片之间可能相差 20 °C。
diag-help-at-fw = 与 RP2354B 的不同时为橙色：正在重新烧写，或烧写失败。
diag-help-bt-module = KVM 烧写它时为橙色（更新后，10 到 20 秒）。本次启动未运行其固件时为红色（下次通电时重试），连续 5 次失败后也为红色：之后只有在通电时按住 KVM 按键，或“固件 → 重试”，才会再次尝试。
diag-help-bt-fw = 模块运行 KVM 携带的固件：更新 KVM 也会更新它。
diag-help-bt-link-errors = 自启动以来与模块交换失败的次数，包括烧写。
diag-help-bt-hosts = 当前已连接并可接收输入的已配对主机。

## 在命令行上输入的设置

parse-colour = 颜色“{ $value }”：请用 #rrggbb
parse-hotkey = 热键“{ $value }”：{ $error }（试试 double:scrolllock 或 ctrl+alt+1）
parse-host-name = 名称“{ $value }”：最多 { $max } 字节（空：主机 n）
parse-profile-name = 名称“{ $value }”：1 到 { $max } 字节
parse-mode = 模式“{ $value }”：{ $modes }
parse-profile-modes = “{ $value }”：键盘、鼠标和其他端口的三个模式，例如 auto,auto,passthrough
parse-number = “{ $value }”：不超过 { $max } 的数字
parse-one-of = “{ $value }”：请用 { $choices }
parse-unknown-setting = 未知设置

## 窗口：侧栏和标题

window-title-sim = { -product }（模拟）
simulated-kvm = 模拟 KVM
page-overview = 概览
page-ports = 端口
page-displays = 显示器
page-keyboard = 键盘
page-settings = 设置
page-firmware = 固件
page-wireless = { -wireless }
light-mode = 浅色模式
dark-mode = 深色模式
no-kvm-connected = 未连接 KVM
leds = 指示灯
active-pc = 当前电脑
profile = 方案
connected-to = 已连接 · { $device }
reading = 正在读取…
not-connected = 未连接

## 概览

chip-active = 当前
chip-this-computer = 本机
pc-vbus = VBUS { $volts } V · 提供 { $offers }
showing = 显示
switch-here = 切换到此
front-ports = 前置端口
unnamed-device = （未命名设备）
host-has-input = { $host } 通过{ -wireless(case: "lower") }获得键盘和鼠标。直通的端口仍留在 { $pc }。
power = 电源
power-left = 来自{ $source }，前置端口约剩 { $milliamps } mA
budget-low = 所接设备可能需要更多电量：请使用 PWR 端口。
selftest-line = 自检{ $result }
recent = 最近

## 端口

port-keyboard = 键盘端口
port-mouse = 鼠标端口
port-other = 其他端口
port-short-keyboard = 键盘
port-short-mouse = 鼠标
port-short-other = 其他
redetect = 重新检测
mode = 模式
no-device = 无设备
port-module-summary = { -wireless }模块，不用于电脑
port-module-note = 此端口上是 KVM 的{ -wireless }模块（一块 ESP32-C3 开发板）：它为{ -wireless }主机服务，不连接任何电脑。拔下该开发板即可再次将此端口用于设备。若要从电脑访问您自己的 ESP32 开发板，请将此端口设为直通模式：ESP32 开发板将原样连接到电脑。
running-mode = 以{ $mode }模式运行
up-to-ma = 最多 { $milliamps } mA
over-current-faults = { $count } 次过流故障
profiles = 方案
profiles-note = 每个方案是一组端口模式，例如工作时用模拟设备，游戏时用直通。长按 KVM 上的按键选择下一个方案（键盘页）。下面的端口运行当前方案。
name = 名称
profiles-in-use = 使用中的方案：
remember-profile = 通电时使用上次的方案（否则使用方案 0，选择方案时不写入 KVM 的闪存）
config-locked = 已在 KVM 上关闭通过软件配置。
config-locked-how = 重新开启：设置 → 软件控制，然后按 KVM 上的按键。

## 键盘

hotkeys = 热键
hotkey-toggle = 切换电脑
hotkey-pc1 = 选择 PC1
hotkey-pc2 = 选择 PC2
hotkey-next = 下一个目标
cant-learn-firmware = 此 KVM 的固件无法学习热键：请直接输入热键
cant-learn-not-active = 只有当前电脑可以学习热键：请先将 KVM 切换到这台电脑
cant-learn-no-keyboard = 没有插入 KVM 可读取的键盘
learning-another = 正在学习另一个热键
learned-with-warning = { $warning } 应用时仍会使用它。
learned = 已从键盘学习。应用时使用它。
not-learned = 未学习：{ $why }。
kvm-no-answer = KVM 未应答。
learn-hint-sim = 或者按“学习”，然后在此窗口中按热键：它会模拟 KVM 的键盘。
learn-hint = 或者按“学习”，然后在 KVM 的键盘上按热键。
press-the-hotkey = 请按热键… { $seconds } 秒
apply = 应用
cancel = 取消
learn = 学习
learn-hover = 在 KVM 的键盘上按热键，而不是输入它
passthrough-keyboard-note = 直通模式的键盘直接连到电脑：KVM 看不到它，因此热键在它上面无效，也无法从它学习热键。
double-tap-window = 毫秒内双击
kvm-key = KVM 上的按键
tap = 轻按
long-press = 长按
press-tap = 轻按
press-long-from = 按住 { $seconds } 秒起为长按
held-for-long-press = 按住长按：
kvm-key-note = 无论此处如何设置，按一下都会确认等待中的固件更新；待机时第一次按只会唤醒 KVM。
keymap-vial = 键位映射 (Vial)
keymap-note = 在显示其接口时，用 Vial 应用编辑键位映射。关闭接口后键位映射仍然有效。更改接口会让两台电脑重新检测 KVM（约 1 秒）。
vial-interface = Vial 接口：
vial-off = 关
vial-auto = 自动（没有镜像设备需要该端点时）
vial-always = 始终
apply-keymap = 将键位映射应用于 KVM 标准化的键盘

## 设置

this-computer = 本机
autostart = 随系统启动，最小化到托盘
could-not-change = 无法更改：{ $error }
appearance = 外观：
theme-system = 跟随系统
theme-dark = 深色
theme-light = 浅色
language = 语言：
language-system = 跟随系统
brightness = 亮度
pc-colour = { $pc } 颜色
pc-side = 电脑侧
polling = 轮询：
polling-125us = 125 µs（延迟最低）
polling-1ms = 1 ms（适用于挑剔的 BIOS）
power-cycle = 直通设备切换电脑时重新上电
standby = 电脑关机或睡眠时
standby-note = 没有电脑使用 KVM 10 秒后，其指示灯熄灭，前置端口断电。开机的电脑或按一下 KVM 上的按键会唤醒它。
auto-select = 跟随开机的电脑：当前电脑关机时，另一台成为当前电脑
standby-wake = 电脑睡眠时保持键盘和鼠标供电，以便按键唤醒它
standby-glow = 待机时让按键发光（否则所有指示灯熄灭）
save-now = 立即保存
factory-settings = 恢复出厂设置
unsaved-changes = 未保存的更改将在几秒后写入。
software-control = 软件控制
software-control-note = 任一台电脑都可以关闭这些选项。重新开启需要 KVM 上的按键：在此处请求后 { $seconds } 秒内按一下，或在 KVM 通电时按住按键。
allow-switching = 允许通过软件切换
allow-config = 允许通过软件配置
not-turned-on = 未开启：没有及时按下 KVM 上的按键。
press-key-to-confirm = 请按 KVM 上的按键确认（剩 { $seconds } 秒）。其指示灯以白色脉动。
confirm-lock-switching = 关闭通过软件切换？
confirm-lock-config = 关闭通过软件配置？
turn-off = 关闭

## 固件

versions = 版本
wireless-module = { -wireless }模块
hardware = 硬件
serial = 序列号
uptime = 运行时间
gave-up-note-chip = 连续 5 次失败后 KVM 停止了尝试，以免每次通电都重新烧写芯片。有新固件时、通电时按住其按键时，或现在，它会再次尝试：
try-again = 重试（KVM 将重启）
update = 更新
image-file = 镜像文件：
install = 安装
press-key-to-install = 请按 KVM 上的按键以安装更新。
installed = 已安装；KVM 将重启。
fw-file-title = 固件文件
fw-file-hint = 发布版的文件夹、kvm-rp2354.bin 或 .uf2 文件
browse = 浏览…
fw-shipped = 随附固件
fw-shipped-note = 随本应用附带
fw-file-is = 固件 { $version }，适用于硬件 { $major }.x
fw-same = KVM 已在运行此版本。
fw-newer = 比 KVM 的 { $kvm } 新。
fw-older = 比 KVM 的 { $kvm } 旧：安装它会回退版本。
fw-no-file = 未选择固件：请选择文件或发布版的文件夹。
update-over-pc-note = 通过本机的端口，更新正在运行的 KVM。新固件以试用方式启动：如果它没有正常启动，KVM 会回到旧固件。
pick-firmware = 选择 KVM 的固件
firmware-files = KVM 固件
boot-title = 处于引导程序中（PWR/SERVICE 端口）
boot-kvm = 固件为 { $version } 的 { -product }；更新将写入分区 { $slot }。
boot-kvm-empty = 没有固件的 { -product }；将写入分区 { $slot }。
boot-new = 新板子：上面还什么都没有。设置时写入分区表和固件。
boot-new-unpartitioned = 固件 { $version }，但没有分区表（由调试器烧写）。设置时两者都写入。
boot-other = 不是 { -product }：是带有其他分区表或其他固件的 RP2350。不会在其上安装任何东西。
boot-unknown-note = 没有它，本应用无法区分 { -product } 和新板子：只在有固件的 { -product } 上安装，只设置新板子。
boot-drive = 驱动器：{ $path }
boot-no-drive-note = 其驱动器未挂载：安装时会挂载它，或先在文件管理器中打开它。
boot-set-up = 设置
boot-writes = 写入 { $file }：固件 { $version }，适用于硬件 { $major }.x
boot-writing = 正在写入
boot-restart = 不安装直接重启
boot-restart-hover = 退出引导程序：板子启动其现有固件。
boot-installed = 已安装：板子将以新固件重启。将其连接到电脑即可使用。
boot-none-title = 通过 PWR/SERVICE 端口安装
boot-none-note = 适用于新板子，或固件无法启动的板子：将本机连接到 KVM 的 PWR/SERVICE 端口。新板子会自动出现在这里；其他板子需在插入时按住其针孔按钮。
boot-found-hint = 一台 { -product } 正在 PWR/SERVICE 端口上的引导程序中等待。
open-firmware-page = 固件页
identify = 识别（闪烁指示灯）
diagnostics = 诊断
measuring = 正在测量…
diag-bad = { $count } 项异常
diag-warnings = { $count } 项警告
diag-all-good = 全部良好
diag-readings = { $count } 项读数，{ $seconds } 秒前
copy = 复制
copy-hover = 将读数复制为文本
looking-for-kvm = 正在寻找 KVM…
plug-in-kvm = 请将 KVM 连接到本机。
udev-hint = 从源码安装？请安装 software/odkvmctl/udev/70-opendyn-kvm.rules（见 DEVELOPMENT.md）。

## 无线主机（蓝牙功能；其名称为 -wireless 术语）

host-connecting = 正在连接…
host-hover = { -wireless }，{ $state }
send = 发送
cancel-pairing = 取消配对
host-pairing = 一台{ -wireless(case: "lower") }主机正在配对
passkey-text = 主机显示六位数字：在 KVM 的键盘上输入并按回车（Esc 取消），或在此处输入。在此之前，KVM 的键盘不会向其他地方输入。
passkey-typed = 在 KVM 的键盘上：{ $digits }
module-is = 模块状态：{ $state }。
module-programming-note = 固件更新后，KVM 用其携带的固件烧写模块，需要 10 到 20 秒。完成后{ -wireless }主机会重新连接。
module-failed-note = { -wireless }已关闭，直到 KVM 重新通电，届时会再试一次。两台电脑照常工作。
gave-up-note-module = 连续 5 次失败后 KVM 停止了尝试，以免每次通电都重新烧写模块。有新固件时、通电时按住其按键时，或现在，它会再次尝试：
module-on-port = 该模块是{ $port }上的一块 ESP32-C3 开发板。
module-failed-note-board = 该开发板未运行模块固件，因此{ -wireless }会一直关闭，直到 KVM 重启。两台电脑照常工作。现在再试一次：
gave-up-note-board = 连续 5 次失败后 KVM 停止了尝试，以免每次插入都重新烧写开发板。KVM 重启时，或现在，它会再次尝试：
pair-new-host = 配对新主机
pair-note = 让 KVM 以“{ -product }”的名称可被发现一分钟。在电脑、平板或手机的{ -host-pairing-settings }中将其作为键盘配对。如果主机显示六位数字，请在 KVM 的键盘上输入并按回车。
pair = 配对
slots-full = 请先忘记一台主机：四个槽位都已占用
discoverable = 可被发现 · { $seconds } 秒
stop = 停止
hosts = 主机
hosts-note = { -wireless }主机获得 KVM 自己的键盘和鼠标（镜像、身份和直通仍留在电脑上）。“保持连接”让切换即时完成；“仅在选中时”适合连接键盘时会隐藏屏幕键盘的手机和平板。
slot-free = 空闲
active = 当前
forget = 忘记
forget-hover = 删除配对。若要重新配对，也请在主机上删除“{ -product }”。
link-every = 每 { $ms } 毫秒
link-may-skip = 可跳过 { $count } 次
link-signal = 信号 { $dbm } dBm
policy-stay = 保持连接
policy-on-select = 仅在选中时
a-host = 一台{ -wireless(case: "lower") }主机

## 托盘

tray-not-connected = { -product }：未连接
tray-host-active = { -product }：{ $host } 为当前（{ -wireless(case: "lower") }）
tray-pc-active = { -product }：{ $pc } 为当前
tray-switch-to = 切换到 { $target }
tray-open = 打开
tray-quit = 退出

## 最近事件

ev-switched = 已切换到 { $target }（{ $source }）
ev-port-changed = { $port }已更改
ev-fault-port = { $port }：{ $fault }
ev-fault = 故障：{ $fault }
ev-setting-changed = 设置 { $name } 已更改
ev-hotkey-learned = 已学习热键：{ $hotkey }
ev-hotkey-not-learned = 学习热键：{ $why }
ev-pairing-open = { -wireless }配对已开启
ev-pairing-closed = { -wireless }配对已关闭
ev-passkey-wanted = 一台{ -wireless(case: "lower") }主机请求其配对码
ev-paired = 已配对：{ $host }（槽位 { $slot }；在{ -wireless }页面为其命名）
ev-forgot = 已忘记{ -wireless(case: "lower") }主机 { $host }
ev-host-connected = { $host } 已连接
ev-host-disconnected = { $host } 已断开
ev-switched-host = 已切换到 { $host }（{ -wireless(case: "lower") }）
ev-monitor-not-switched = 显示器未切换：{ $error }
fault-over-current = 过流
fault-enumeration = 枚举失败
fault-link-down = 链路断开
fault-power-budget = 电量不足
fault-hub = 集线器错误
no-diagnostics = 此 KVM 的固件尚无诊断功能：请更新。
kvm-refused = KVM 拒绝了：{ $why }。

## 显示器：跟随 KVM 的显示器（DDC/CI）

profile-n = 方案 { $n }
arrangement-n = 布局 { $n }
display-follow = 跟随 KVM
display-pc1 = 保持在 PC1
display-pc2 = 保持在 PC2
display-off = 不切换
input-none-pc = — 不切换
input-none-host = — 保持在电脑上
display-switched = { $monitor } 已切换到 { $input }
displays-title = 显示器输入跟随 KVM
displays-note = 当 KVM 切换电脑（或切换到{ -wireless(case: "lower") }主机）或方案时，本机会切换下面的显示器（DDC/CI）：每个方案都可以让显示器跟随 KVM、保持在某台电脑上，或不去管它。KVM 会把每次更改告诉两台电脑，因此尽量只在一台电脑上设置每台显示器。
displays-two-pcs = 两台电脑不共享这些设置：在两台电脑上都设置的显示器会收到两次命令，如果设置不同，它不会跟随任何一边。如果显示器能在未显示的输入上接收命令，一台电脑就够了：从本机切到“显示 PC1”/“显示 PC2”再切回即可得知。如果切不回来，请在两台电脑上做相同的设置。
displays-ddc-on = 必须在显示器自己的菜单中开启 DDC/CI。
look-again = 重新查找显示器
asking-monitors = 正在询问显示器…
this-computer-is = 本机是 { $pc }
kvm-not-connected = 未连接 KVM
no-monitor = 没有显示器应答。请检查显示器菜单中是否开启了 DDC/CI。
saved-not-connected = 已保存，当前未连接
input-other = 其他
input-other-hover = 显示器未列出的输入，用十六进制表示（例如 0x0F），然后按回车
use-current = 使用当前
use-current-hover = 此显示器当前显示的输入
show-target = 显示 { $target }
monitor-shows = 显示 { $input }
not-answering = 无应答
target-input = { $target } 输入
displays-hosts-note = { -wireless }主机是可选的：当其中一台拥有键盘和鼠标时，跟随 KVM 的显示器如果在此处有它的输入就显示它，否则保持在电脑上。
in-each-profile = 在每个方案中：
input-missing = 某个方案需要的输入尚未在上面选择。
could-not-save = 无法保存本机的设置：{ $error }
input-unknown = 输入 { $value }
ddc-no-answer = 无应答（显示器菜单中是否开启了 DDC/CI？）
ddc-garbled = 应答乱码
ddc-unexpected = 意外的应答
ddc-unsupported-code = 显示器不支持
ddc-not-connected = { $id }：未连接
ddc-unsupported-os = 仅 Windows 和 Linux 支持 DDC/CI
ddc-no-access = { $path }：无权访问。请安装软件包，或按 README 安装 udev 规则，然后重新登录。
ddc-no-i2c-dev = { $path }：不存在，i2c-dev 模块未加载。请安装软件包，或按 README 加载它。

## 与 KVM 通信

error-timeout = KVM 未应答
error-device = KVM 拒绝了：{ $why }
error-protocol = 协议错误：{ $error }
protocol-unexpected = 意外的应答 { $answer }
protocol-update-ended = 更新以状态 { $state } 结束
error-not-confirmed = 没有及时按下 KVM 上的按键：未做任何更改
host-n = 主机 { $n }
hid-open-linux = { $error }（安装 software/odkvmctl/udev/70-opendyn-kvm.rules 以便无需 root 访问）
no-kvm-found = 未找到 { -product }

## 固件文件

img-damaged-uf2 = UF2 文件已损坏
img-uf2-partition-table = 此 UF2 还包含分区表（kvm-first.uf2）；请使用 kvm-rp2354.uf2 或 kvm-rp2354.bin
img-uf2-other-chip = 用于其他芯片的 UF2（family { $family }）
img-uf2-out-of-range = 地址 { $address } 处的 UF2 块超出范围
img-too-big = { $kb } KB 超过固件分区的容量（{ $max } KB）
img-not-rp2354b = 不是 RP2354B 固件（没有 RP2350 镜像定义）
img-wrong-kind = 类型错误的 RP2350 镜像（标志 { $flags }）
img-no-fw-id = 不是 { -product } 固件（没有固件 ID）
img-other-hardware = 固件 { $version } 适用于硬件 { $major }.x，此 KVM 是硬件 { $hw }
img-not-first = 不是首次安装用的 UF2（kvm-first.uf2：分区表和固件）
fw-read = { $file }：{ $error }
fw-not-in-folder = { $folder } 中没有 { $name }
boot-no-picoboot = 启动 ROM 没有 PICOBOOT 接口
boot-no-permission = 无权访问 RP2350 启动 ROM（USB 2e8a:000f）：请安装 udev 规则（software/odkvmctl/udev/70-opendyn-kvm.rules）
boot-no-access = 无法打开 RP2350 启动 ROM：{ $error }
boot-usb = USB：{ $error }
boot-rom-ignored = 启动 ROM 忽略了该文件（不适用于此板子的分区）
boot-rom-locked = 另一个程序占用了启动 ROM（picotool？）
boot-rom-bad-address = 启动 ROM 已停止：地址超出闪存范围
boot-rom-write-error = 启动 ROM 无法写入闪存
boot-rom-reboot-failure = 启动 ROM 已写入固件，但无法启动它
boot-no-drive = 启动 ROM 的驱动器（RP2350）未挂载：请在文件管理器中打开它，然后重新安装
boot-write = 写入启动 ROM 的驱动器：{ $error }
boot-no-restart = 复制后板子没有重启（20 秒内无反应）

## odkvmctl：帮助

cli-about = 控制 { -product } 切换器
cli-sim = 与内置的模拟 KVM 通信，而不是真实设备
cli-status = 显示 KVM 的状态（默认）
cli-diag = 测量 KVM 的时钟、电压、温度和 USB 链路，每项评为良好、警告或异常；有任何异常时以 1 退出
cli-list = 列出本机可见的 KVM 控制接口
cli-watch = 实时打印事件（切换、端口变化、故障）
cli-switch = 将键盘、鼠标和其他设备切换到一台电脑，或（有{ -wireless(case: "lower") }模块时）切换到一台{ -wireless(case: "lower") }主机
cli-wireless = { -wireless }主机（带模块的 KVM）：列出（默认）、配对新主机、忘记、命名，或设置其他目标为当前时主机的状态
cli-port = 显示前置端口、设置其模式，或重新检测其设备
cli-profile = 列出方案、激活一个（profile next、profile 1、profile gaming），或修改一个（profile 1 name gaming、profile 1 mouse passthrough）
cli-profile-which = 方案的编号或名称，或 next
cli-profile-setting = name、arrangement，或端口：keyboard、mouse、other
cli-hotkey = 显示或设置热键：double:scrolllock、double:rctrl:noswallow、ctrl+alt+1、none；或从 KVM 键盘上接下来按下的键学习
cli-config = 读取和更改设置
cli-identify = 闪烁 KVM 的指示灯
cli-update = 安装 RP2354B 固件：如果 PWR/SERVICE 端口上有板子在引导程序中等待，就装到它上面，否则通过电脑端口安装（需要按一下 KVM 上的按键）
cli-reboot = 重启 KVM，或让某个芯片进入其引导程序
cli-display = 本机可通过 DDC/CI 切换输入的显示器（不需要 KVM）
cli-display-list = 显示器、各自显示的输入及其提供的输入（默认；需要几秒钟）
cli-display-input = 将显示器切换到某个输入，例如 display input XYZ-1A2B-1234 0x11（ID 来自 display list）
cli-hosts-list = 四个主机槽位：名称、状态、策略
cli-hosts-pair = 让 KVM 以“{ -product }”可被发现 60 秒；然后在电脑、平板或手机上配对。如果主机显示六位数字，请在 KVM 的键盘上输入并按回车（或使用 passkey）
cli-hosts-pair-stop = stop 提前关闭配对
cli-hosts-passkey = 配对主机显示的六位数字，在手边没有 KVM 键盘时使用
cli-hosts-forget = 忘记主机的配对：槽位 1-4，或 all
cli-hosts-name = 为主机槽位命名（空：“主机 n”）
cli-hosts-policy = stay：主机保持连接，切换到它是即时的。on-select：仅在选中时连接（适合连接键盘时会隐藏屏幕键盘的手机和平板）
cli-port-redetect = 重新检测设备（在直通端口上更换设备后需要）
cli-config-get = 显示一项设置，或全部设置
cli-config-set = 更改设置（名称和当前值见 config get）
cli-config-save = 立即将设置写入 KVM 的闪存（否则在最后一次更改 5 秒后写入）
cli-config-reset = 恢复出厂设置（保留软件锁定设置）
cli-target-next = 下一个目标：PC1、PC2，然后是每台已连接的{ -wireless(case: "lower") }主机
cli-reboot-at32-retry = 给 AT32F405 最多再 5 次烧写机会，然后重启
cli-reboot-wireless-retry = 给{ -wireless(case: "lower") }模块最多再 5 次烧写机会，然后重启

## odkvmctl：输出

cli-port-auto = 自动：{ $why }
cli-port-set-to = 设为{ $mode }：{ $why }
cli-status-head = { -product }（{ $device }）：固件 { $firmware }，AT32 { $at1 } / { $at2 }，硬件 { $hardware }，序列号 { $serial }
cli-module-firmware = { -wireless }模块：固件 { $version }
cli-module-state = { -wireless }模块：{ $state }
cli-module-port = { -wireless }模块：{ $port }上的一块 ESP32-C3 开发板
cli-active-host = 当前：{ -wireless(case: "lower") }主机 { $slot }“{ $host }”（所属电脑 { $pc }）
cli-active = 当前：{ $target }
cli-this-computer = 本机是 { $pc }
cli-profile-line = 方案：{ $n }（{ $name }）
cli-host-slot = 主机 { $slot }
cli-pairing-open = { -wireless }配对已开启（剩 { $seconds } 秒）
cli-passkey-waits = 一台配对中的主机在等待其配对码：在 KVM 的键盘上输入，然后按回车
cli-budget-low = 设备可能需要更多电量
cli-power = 电源：{ $power }
cli-selftest = 自检：{ $result }
cli-try-again = 重试：{ $command }，或在 KVM 通电时按住其按键
cli-ports = 端口：
cli-switching-disabled = 已禁用通过软件切换
cli-config-disabled = 已禁用通过软件配置
cli-locked = 已在设备上锁定：{ $what }
cli-host-ready = 已连接，{ $ms } 毫秒
cli-profiles-in-use = 使用中 { $count } / { $max }：odkvmctl config set profiles <1-{ $max }>
cli-no-profile = 没有方案“{ $which }”：0 到 { $last } 的数字或名称
cli-learn-sim = 无法从这里按模拟 KVM 的键盘：请在 `opendyn-kvm --sim` 中试试“学习”
cli-learn-press = 请在 { $seconds } 秒内在 KVM 的键盘上按新热键。在此期间按键不会到达任何电脑。
cli-learn-unsupported = 此 KVM 的固件无法学习热键：请以文本给出热键，例如 ctrl+alt+1
cli-nothing-changed-why = { $why }：未做任何更改
cli-use-it-for = 将它用于 { $name }？[y/N]
cli-turn-it-off = 关闭它？[y/N]
## 除 y 和 yes 之外，回答 [y/N] 问题表示“是”的词，用逗号分隔。
cli-yes-words = y, yes, 是, 好
cli-readings = { $count } 项读数：{ $verdict }
cli-ev-back-on-pc = 输入回到电脑
cli-ev-host-active = 已切换到{ -wireless(case: "lower") }主机 { $slot }
cli-ev-pairing-changed = { -wireless }配对已更改
cli-ev-host-changed = { -wireless }主机 { $slot } 已更改
cli-no-monitors = 未找到显示器
cli-inputs = 输入：{ $inputs }
cli-input-number = 输入“{ $value }”：0x11 或 17 这样的数字
cli-no-interface = 未找到 { -product } 控制接口
cli-connecting = 正在连接 KVM
cli-watching = 正在监视 { $device }（按 Ctrl+C 停止）
cli-no-module = 此 KVM 没有{ -wireless(case: "lower") }模块
cli-no-host-in-slot = 该槽位没有配对的主机
cli-no-module-boards = 此 KVM 没有{ -wireless(case: "lower") }模块（0.16.0 起的板子可以装一个）
cli-pair-usage = “{ $value }”：请用 `wireless pair` 或 `wireless pair stop`
cli-pairing-opened = 配对已开启 { $seconds } 秒：在电脑、平板或手机上配对“{ -product }”。
cli-pairing-digits = 如果它显示六位数字，请在 KVM 的键盘上输入并按回车。
cli-six-digits = 主机显示的六位数字
cli-no-passkey-wanted = 没有主机在等待配对码
cli-slot-or-all = 槽位 1-4 或 all
cli-slot = 槽位 1-4
cli-no-profiles = 此固件没有方案
cli-profile-not-in-use = 方案 { $n } 未在使用：odkvmctl config set profiles { $count }
cli-missing-value = profile { $setting }：缺少值
cli-nothing-changed = 未做任何更改。
cli-unknown-setting = 未知设置“{ $name }”
cli-press-key-within = 请在 { $seconds } 秒内按 KVM 上的按键确认…
cli-saved = 已保存
cli-factory-restored = 已恢复出厂设置
cli-reading = 正在读取 { $file }
cli-firmware-for = 固件 { $version }，适用于硬件 { $major }.x
cli-update-file = 固件：发布版的文件夹、kvm-rp2354.bin 或 .uf2 文件（默认：本程序附带的固件）
cli-boot-no-drive = 驱动器未挂载
cli-no-firmware = 未指定固件，本程序也没有附带固件
cli-no-kvm-anywhere = 电脑端口上没有 { -product }，PWR/SERVICE 端口上也没有处于引导程序中的
cli-boot-several = 有多块板子处于引导程序中：请一次只连接一块
cli-boot-unknown = 没有它，odkvmctl 无法区分 KVM 和新板子。请手动将 kvm-rp2354.uf2（有固件的 KVM）或 kvm-first.uf2（新板子）复制到其 RP2350 驱动器上。

## 跨屏（功能名称为 -hop 术语）

page-screens = { -hop }
switch-source-edge = 指针越过了屏幕边缘
hop-line = { -hop }：{ $state }
hop-state-off = 在此方案中关闭
hop-state-active = 开
hop-state-no-absolute = 需要绝对指针
hop-state-not-normalized = 需要鼠标端口处于标准化模式
hop-state-no-screens = 当前电脑在此方案的布局中没有屏幕
hop-state-boot-protocol = 电脑处于固件设置界面时关闭
hop-state-wireless = { -wireless(case: "lower") }主机拥有输入时关闭
accel-flat = 平直（仅速度）
accel-precision = 精确曲线，同 Windows
accel-adaptive = 自适应，同 Linux
mapping-desktop = 覆盖其所有屏幕（Linux）
mapping-main-screen = 仅在其主屏幕上（Windows）
units-physical = 物理像素（Windows、X11）
units-logical = 逻辑像素（Wayland）
bt-pointer-absolute = 绝对（精确）
bt-pointer-relative = 相对（用于不支持绝对指针的系统）
parse-screen = “{ $value }”：屏幕写作 pc1 2560x1440 150% at 1920,0 desk 0,0（{ -wireless }主机用 host1–host4；缩放和位置可省略；加 no-hop 指针不进入该屏幕，加 fixed 保持其在电脑上的位置），或 off
parse-range = “{ $value }”：{ $min } 到 { $max } 的数字
screens-none-found = 未找到屏幕
screens-xrandr-missing = 无法运行 xrandr 来列出屏幕（{ $error }）
screens-xrandr-failed = xrandr 无法列出屏幕：{ $error }
screens-unsupported-os = 仅 Windows 和 Linux 支持查找屏幕
profile-hop = 跨屏
profile-hop-hover = 此方案中的{ -hop }（有单独的页面）
hop-firmware-too-old = 此 KVM 的固件没有{ -hop(case: "lower") }功能：请更新。
hop-what = KVM 自己移动指针，跨越下面布置的屏幕。将指针推过边缘到另一台电脑的屏幕上（另一台电脑或{ -wireless }主机），KVM 就会切换到它，键盘也一起切换。
hop-turn-on-absolute = 开启绝对指针
hop-mouse-normalized = 在此方案中让鼠标端口以标准化模式运行
hop-in-profiles = 每个方案的布局
hop-profile-active = { $name }（当前）
hop-absolute-pointer = KVM 鼠标上的绝对指针
hop-absolute-pointer-note = { -hop }需要它。开启或关闭它会让两台电脑重新连接 KVM 的设备一次。
hop-arrangement = 布局
hop-arrangement-note = 将屏幕拖到它们实际摆放的位置。指针在两个屏幕相接处通过，并跳过它们之间的空隙；旁边没有屏幕的边缘会挡住指针。
hop-overlap = 有些屏幕重叠：指针无法在它们之间穿越。连接两台 PC 的显示器：在不显示它的 PC 上关闭其{ -hop(case: "lower") }，并把它放在另一台 PC 的屏幕上。
hop-add-screen = 添加 { $pc } 的屏幕
hop-detect = 检测 { $pc } 的屏幕
hop-detect-hover = 本机的屏幕，包括其分辨率、缩放和在显示设置中的位置
hop-remove-screen = 删除
hop-draft = 尚未应用：在你应用之前，KVM 保持其现有布局。
hop-applying = 已应用：正在等待 KVM 确认。如果被拒绝，修改会保留在这里以便重试。
hop-apply = 应用
hop-discard = 放弃修改
hop-mapping-of = { $pc } 映射指针
hop-units-of = { $pc } 显示设置的单位
hop-arrangement-none = 无（关闭）
hop-arrangement-in-use = { $name }（使用中）
hop-arrangement-name = 名称
hop-bt-pointer-of = { $host } 的指针
hop-bt-pointer-note = { -wireless }主机和电脑一样得到绝对指针：其光标正好位于 KVM 指针所在之处。不接受绝对指针的系统（部分平板和手机）需要选“相对”：指针进入该主机时，KVM 先把光标推到左上角，再从那里移动。主机会对这种移动加速，因此其光标会偏离 KVM 的指针，直到被推到某个边缘。
hop-mapping-note = Windows 只把绝对指针放在其主屏幕上。KVM 像鼠标一样将光标移到其他屏幕，而 Windows 会对此加速：要让边缘精确，请在这样的电脑上关闭“提高指针精确度”，或让 { -product } 在其上保持运行。
hop-mapping-windows = 本机运行 Windows，它只把绝对指针放在主屏幕上：若覆盖所有屏幕，指针会落在错误的位置，也无法到达其他屏幕。请选择“仅在其主屏幕上（Windows）”。
hop-no-screens = 还没有屏幕：为每台电脑添加一个。
hop-main-screen = 主屏幕
hop-screen-hover = 单击编辑（再次单击选择下面的屏幕），拖动移动
hop-screen-pc = 电脑
hop-screen-resolution = 分辨率
hop-screen-scale = 缩放
hop-screen-desk = 在其电脑上的位置
hop-screen-at = 在此处的位置
hop-screen-used = { -hop }会进入此屏幕
hop-screen-used-note = 对于连接两台 PC 且显示另一台的显示器请关闭：把它放在那台 PC 的屏幕上。其 PC 仍计入它，因此指针会落在其他屏幕上的正确位置。
hop-screen-off-short = 不参与{ -hop(case: "lower") }
hop-screen-desk-note = 屏幕在其电脑显示设置中的位置，以那台电脑的像素为单位（在 Wayland 桌面上为逻辑像素）。在 Windows 上，主屏幕位于 0, 0。
hop-detected = { $pc } 现在有 { $count } 个屏幕，与本机相同。{ $pc } 映射指针：{ $mapping }。
hop-pointer = 指针
hop-accel = 加速
hop-speed = 速度
hop-resistance = 边缘阻力
hop-corner = 角落
hop-pointer-note = 电脑不会对绝对指针加速，因此由 KVM 来加速。边缘阻力是指针必须抵着边缘推多远才会过到另一台电脑；角落是在该边缘两端多远的范围内永远不会过去，以便角落仍然容易点到。拖动永远不会过去。
cli-screens = 跨屏：各电脑屏幕的布局以及指针如何移动
cli-screens-list = 显示各布局和指针设置
cli-screens-detect = 将本机的屏幕放入布局
cli-screens-which-pc = KVM 没有说明本机是哪台电脑（模拟器或服务端口）：请改用 odkvmctl config set arrangement0-screen0 … 设置屏幕
cli-hop-mapping = { $pc } 映射指针：{ $mapping }
cli-hop-units = { $pc } 的显示设置以{ $units }计
cli-hop-pointer = 指针：{ $accel }，速度 { $speed }，边缘阻力 { $resistance } px，角落 { $corner } px
cli-screens-arrangement = 布局，用编号（0–3）或名称；默认为当前方案的布局
cli-no-arrangement = 没有布局“{ $which }”：请用 0 到 3 的数字或名称
cli-arrangement-unused = 不在任何方案中
cli-arrangement-used = 用于 { $profiles }
cli-hop-bt-pointer = { $host } 的指针：{ $pointer }
cli-profile-no-hop = 无{ -hop }
cli-profile-hop = { -hop }：{ $name }
hop-out-of-date = 本机的显示设置与 KVM 中其屏幕的记录不同，因此指针在这里会落到错误的位置：请重新检测其屏幕。
cli-hop-out-of-date = { hop-out-of-date }（odkvmctl screens detect）
