# 来源与修改声明 / Provenance and modifications

本仓库是**个人自用的修改版**，不是上游官方产物，与上游作者无隶属或背书关系。

## 上游

- `SagerNet/sing-box` — <https://github.com/SagerNet/sing-box>，GPL-3.0，© nekohasekai
- 本仓库派生自 `LIghtJUNction/sing-box` 的发布线 `ff669d45`

## 本仓库的修改

- 基于发布线 `ff669d45` 变基，保留 fork 的 lx 系列能力；
- Rust 进程扫描器（`experimental/lx_rust_proc`，rust2 变体）并接入 `common/process`；
- Android 构建标签裁剪：移除无消费者的 `with_lxd`，Android 产物带 `with_lx_rust_proc_rust2`；
- 惰性进程路径解析：没有 `process_name` / `process_path` 规则时不再扫描 `/proc`；
- 扫描器 crate 声明独立 workspace，使其可在模块仓内构建。

## 许可

来自 sing-box 的部分（含上述修改）继续适用 **GPL-3.0**，许可证全文见 `LICENSE`。
本仓库即对应源码；同一分支上的提交包含完整构建脚本与依赖固定信息。

## 作者与致谢

- **sing-box 原作者：nekohasekai / SagerNet** — <https://github.com/SagerNet/sing-box>，GPL-3.0；
  本仓库经 **LIghtJUNction** 的 sing-box fork 派生。
- **MagicNet 原作者 / 维护者：LIghtJUNction** — <https://github.com/LIghtJUNction>，
  个人主页 <https://lightjunction.github.io/lightjunction/>，
  X/Twitter [@LIghtJUNction_x](https://x.com/LIghtJUNction_x)。
  本 fork 只服务于其项目在个人设备上的使用，核心与协议的功劳属于上述两位作者。

## 免责

仅供个人使用，不提供任何担保；使用风险自负。
本仓库不包含订阅、节点凭据、个人配置或设备数据。
