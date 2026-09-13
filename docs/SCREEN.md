# WireGuard 小屏控制

使用 OpenUI → Network → WireGuard →“Open device screen”。该面板临时接管
320×480 触摸屏，使用英文界面；点“Stock UI”、网页“Restore stock UI”，或两分钟无触摸操作后
恢复原厂显示和触摸。关闭网页不会立即退出小屏。退出不会关闭 WireGuard。

首页显示配置状态、最近握手、隧道累计流量和总开关；关闭隧道需确认。
握手正常不代表已经验证互联网可用。配置和密钥仍在网页导入，小屏不显示密钥。
设备页每页四行，勾选代表走 WireGuard；未勾选走正常网络。离线保存项仍可编辑。
批量保存后生效；新设备默认策略继续在网页设置。设备发现沿用网页的邻居、桥接和
无线客户端信息，不是主动在线探测，离线判定可能有延迟。最多显示 128 条记录。

## 构建与安装

屏幕程序单独构建为 aarch64 musl 静态可执行文件，不替换原厂程序或开机入口。
`screen/qpic.h` 基于用户提供的 `mu5250_tweaking` Atomic 示例，程序使用 GPLv3+；
字体使用 GNU Unifont 17.0.05，经 SHA-256 校验转换为内置 BMP 字库（SIL OFL）。
源码和字体许可在 `screen/` 内；与 MIT agent 通过本地 socket 通信。

```sh
ZIG=/path/to/zig-0.14.1 sh screen/build.sh
# 无硬件的实际渲染预览和交互测试：
sh screen/build.sh --preview
# 使用现有部署命令时附加：
# --screen build/screen/openui-screen
```

部署脚本保存组件及验证标记的恢复快照，替换屏幕程序后清除验证标记。
网页入口在本机验证完成前不可用。设备已有运行中的面板时先恢复原厂界面。

**写操作：**以下脚本会临时接管小屏并测试面板崩溃、卡住、网页关闭和
120 秒空闲恢复；不修改 WireGuard 配置。需要现有 SSH 密钥，网页登录密码仅在
内存中读取和使用，不会打印。通过后开放网页入口，失败时撤销入口。

```sh
python3 scripts/verify-screen.py --gateway 192.168.0.1 --port 2222 --ssh-key /path/to/id_ed25519
```

## 接口与运行保护

- 鉴权 HTTP：`GET /api/screen` 返回 `available/verified/running/state`；
  `POST /api/screen` 请求打开；`DELETE /api/screen` 请求恢复。执行异步，轮询状态。
- `PUT /api/wireguard` 返回 `revision`；网页提交 `expected_revision`。
  小屏局部更新使用 `enabled` 或 `devices: [{mac, selected}]`，必须附带版本。
  版本冲突返回 HTTP 409，旧的完整更新请求保留兼容性。
- 私有 Unix socket `/tmp/openui-screen/control.sock` 为 0600，目录为 0700。
  屏幕只接收经过裁剪的状态和设备列表；没有 endpoint、私钥、PSK 或登录凭据。
  变更由 agent 后台执行，小屏轮询作业结果；数据超过十秒未更新时禁用修改。
- 监督进程持有独占锁，先检查原厂启动同步，再记录背光和服务状态。
  原厂界面可能有不受 procd 管理的残留实例；停服务后仅对匹配原厂可执行路径的
  进程发送退出信号，必要时结束残留进程，然后接管 DRM。
- 画面在提交前旋转 180°，原始触摸坐标同时转换为 `(319-x, 479-y)`，匹配机身方向。
- 使用 DRM Atomic 和 RGB565 双缓冲，仅改变内容时刷新。不使用 legacy SETCRTC、
  `image_dump`、电源键截获或厂商 LED ubus 调用。低亮度时临时使用 128/255，退出恢复原值。
- 面板没有心跳超过十秒时由监督进程终止并恢复原厂界面；终止宽限期两秒。
  agent 通信中断十五秒后面板退出。网络任务在 agent 内继续完成。

监督进程恢复失败、同时被强制杀死，或内核显示驱动异常，无法承诺自动恢复。
不要把进程级看护当作内核或断电保护。必要时通过 SSH 执行：

```sh
/data/bin/openui-screen --restore
```

## 验证范围

本地测试覆盖状态解析、实际画面渲染、勾选/确认/保存、版本冲突与设备更新语义。
目标设备的 `--self-test` 使用合成 MT-B 输入测试点击、拖动抑制、事件丢失处理；
`--probe` 只读检查实际触摸坐标范围。自动恢复验证检查原厂进程、同步状态、背光、
网页、默认路由及 WireGuard 配置摘要保持正常。物理观感和手指点击需要在设备屏幕上确认。
WireGuard 服务器的实际出口验证与显示/恢复验证分别进行。
