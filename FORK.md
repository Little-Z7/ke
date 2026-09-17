# 壳（ke）

本仓库是 [herdr](https://github.com/herdrdev/herdr) 的修改版（fork），基于 herdr v0.9.0（提交 b99002ac），遵循 Apache License 2.0（见 LICENSE）。

- 产品名"壳"，安装后的命令名 `ke`，直接运行 `ke` 启动 TUI。与上游 herdr 不是同一产品，不隶属于 herdr 项目，也未获其背书；提到 herdr 只为说明来源。
- cargo 的二进制目标名仍是上游的 `herdr`，因为上游测试与工具依赖 `CARGO_BIN_EXE_herdr`；改名只发生在安装与打包这一步。不要用 `cargo install`，它会装出一个与上游同名的 `herdr`。子命令帮助里的 `herdr <command>` 同样保留上游写法（顶层帮助有说明），以便合并上游。
- 目标：让一个常驻模型（本地或云端）住在终端工作区里，协调各家 coding agent CLI；在 agent 窗格下提供原生输入栏（用户输入先经本机处理，如脱敏，再送入 CLI）；在侧栏显示各 CLI 的额度、任务与常驻模型状态。
- 与已安装的上游 herdr 完全隔离：独立的配置目录、socket 与会话文件，关闭自动更新。窗格内仍注入 `HERDR_ENV` / `HERDR_SOCKET_PATH` / `HERDR_PANE_ID`（指向壳自己的 socket），以兼容各 CLI 已安装的 herdr 状态钩子。

## 安装

macOS 与 Linux（x86_64 / aarch64）：

```
curl -fsSL https://github.com/Little-Z7/ke/releases/latest/download/ke-install.sh | sh
```

- 装到 `~/.local/bin/ke`（`KE_INSTALL_DIR` 可改），下载后按发布里的 `SHA256SUMS` 校验。`KE_RELEASE_TAG=ke-v0.1.0` 可指定版本。更新就是重新运行一次。
- 壳不自带 agent 状态钩子的安装：它复用上游 herdr 为各 CLI 装好的钩子（钩子按窗格里的 `HERDR_SOCKET_PATH` 找 socket，在壳的窗格里就会连到壳）。需要钩子的话先装上游 herdr 并执行 `herdr integration install <agent>`；没有钩子时壳退回到基于屏幕内容的状态检测。
- 输入栏的本机处理进程（脱敏等）和侧栏面板的协调进程不在本仓库里。没有它们时输入栏原文直通，侧栏 ke 面板不显示。
- 从源码安装：`scripts/ke-install.sh`（需要 Rust 与 Zig 0.15.2）。

## 发布

版本号在 `src/build_info.rs` 的 `KE_VERSION`，与上游版本（`Cargo.toml`）分开。发布步骤：改 `KE_VERSION` 并提交，然后

```
git tag ke-v<KE_VERSION> && git push origin ke/main ke-v<KE_VERSION>
```

`.github/workflows/ke-release.yml` 会校验标签与 `KE_VERSION` 一致，构建四个平台的二进制，连同 `ke-install.sh`、`SHA256SUMS`、`LICENSE` 发到 GitHub Release。上游自带的 workflow 只在 `master` 分支和 `v*` 标签上动作，所以壳用 `ke/main` 分支和 `ke-v*` 标签，不要推 `master` 分支或 `v*` 标签到壳的仓库。

## 相对上游的修改

按 Apache-2.0 第 4 条，所有修改过的文件会在此登记，并在文件内标注"Modified by ke"。

| 日期 | 文件 | 修改 |
|---|---|---|
| 2026-09-13 | FORK.md | 新增：fork 说明与修改登记 |
| 2026-09-13 | src/config/io.rs | 应用目录 herdr/herdr-dev → ke/ke-dev（配置、socket、会话、日志、插件与上游分开） |
| 2026-09-13 | src/main.rs | 启动时若在上游 herdr 窗格内（有 HERDR_ENV、无 KE_ENV），清掉继承的 HERDR_* 定位变量；默认配置模板的 worktree 目录 |
| 2026-09-13 | src/pane.rs | 窗格额外注入 KE_ENV=1 |
| 2026-09-13 | src/app/api/plugins/runtime.rs | 插件命令额外注入 KE_ENV=1 |
| 2026-09-13 | src/update.rs | KE_DISABLE_SELF_UPDATE：self_update 直接拒绝 |
| 2026-09-13 | src/app/mod.rs | 后台更新检查受 KE_DISABLE_SELF_UPDATE 控制，始终关闭 |
| 2026-09-13 | src/config/model.rs | version_check / manifest_check 默认关闭；worktree 默认目录 ~/.ke/worktrees（含对应测试断言） |
| 2026-09-13 | src/cli/integration.rs | 禁用 integration install/uninstall，复用上游已装的 CLI 钩子 |
| 2026-09-13 | tests/*.rs | 测试里的应用目录名 herdr-dev → ke-dev |
| 2026-09-13 | src/client/shell/composer.rs（新增） | 原生输入栏：状态、按键编辑、经 Unix socket 调本机处理进程（未配置时原文发送，配置了却失败时拦下不发）、提交到 agent.prompt / pane.send_input、渲染 |
| 2026-09-13 | src/client/shell.rs | 注册 composer 模块 |
| 2026-09-13 | src/client/shell/state.rs | 状态加 composer / composer_hook；layout() 在输入栏打开时给窗格区少算 2 行（新增 base_layout） |
| 2026-09-13 | src/client/shell/input.rs | 按键与文字/粘贴在输入栏打开时交给输入栏 |
| 2026-09-13 | src/client/shell/composition.rs | 画输入栏，光标落在输入栏 |
| 2026-09-13 | src/client/shell/actions.rs | 分派 ToggleComposer |
| 2026-09-13 | src/input/keybindings.rs、src/config/keybinds.rs、src/config/model.rs、src/input/keybind_help.rs、src/main.rs | 新快捷键动作 toggle_composer（默认 prefix+i）及帮助、配置模板 |
| 2026-09-13 | src/client/shell/tests/composer_bar.rs（新增）、tests/mod.rs | 输入栏测试 |
| 2026-09-13 | src/server/client_commands.rs | 客户端界面允许的方法加入 agent.prompt、pane.send_input（输入栏提交用） |
| 2026-09-13 | tests/fixtures/endpoint-method-shapes-v1.json | 只为上面两个新增方法补形状记录，已有方法的记录不变 |
| 2026-09-13 | src/client/shell/ke_panel.rs（新增） | 侧栏 ke 面板：主循环定时器每 2 秒查一次协调进程写的 panel.json（默认 ~/.workcat/ke/panel.json，可用 KE_PANEL_FILE / WORKCAT_KE_HOME 改），修改时间变了才重读；超过 30 秒未更新变暗并标 offline，超过 10 分钟隐藏；渲染只读内存 |
| 2026-09-13 | src/client/shell.rs、src/client/shell/state.rs、src/client/shell/render.rs、src/client/shell/composition.rs | 注册 ke_panel 模块；状态加 ke_panel 与 tick_ke_panel；渲染状态带面板引用 |
| 2026-09-13 | src/client/mod.rs | Timer 分支调用 tick_ke_panel，面板内容变化时重绘 |
| 2026-09-13 | src/client/shell/sidebar.rs、src/client/shell/endpoint_sidebar.rs | agents 区底部切出 ke 面板，保留该区最后一行 |
| 2026-09-13 | src/client/shell/composer.rs、src/client/shell/state.rs、src/client/mod.rs、src/client/shell/tests/composer_bar.rs | 输入栏 M2.1：处理进程改在独立线程调用，30 毫秒内回复的当场发送，慢的由主循环定时器 tick_composer 取回再发；5 秒无回复按拦截处理；处理期间新输入的文字保留；新增 2 个测试 |
| 2026-09-13 | src/client/shell/composer.rs、src/client/shell/tests/composer_bar.rs | 处理进程回复新增 done 动作：处理进程已自行接手（如 @ke 交给常驻模型），不发进窗格、清空输入栏；未知动作仍按拦截处理；新增 1 个测试 |
| 2026-09-17 | scripts/ke-install.sh（新增） | 安装脚本：构建 release 后把 `target/release/herdr` 安装为 `ke`（默认 `~/.local/bin`，`KE_INSTALL_DIR` 可改；`--no-build` 只安装现有二进制；PATH 上没有 zig 且未设 `ZIG` 时回退到仓库旁的 `../ke-tools/zig-*/zig`） |
| 2026-09-17 | src/build_info.rs | 新增 `KE_VERSION`（壳自己的版本号）与 `KE_INSTALL_COMMAND`（安装命令） |
| 2026-09-17 | src/main.rs | `--version` 输出 `ke <KE_VERSION> (based on herdr <上游版本>)`；顶层帮助抬头说明这是 herdr 的修改版、命令名是 `ke` |
| 2026-09-17 | src/update.rs | 拒绝自更新时的提示改为给出安装命令 |
| 2026-09-17 | distribution/ke-install.sh（新增） | curl 安装脚本：从壳的 GitHub Release 下载对应平台二进制，校验 SHA-256，安装为 `ke` |
| 2026-09-17 | .github/workflows/ke-release.yml（新增） | 发布流程：`ke-v*` 标签触发，构建 linux/macos × x86_64/aarch64 并创建 Release |
| 2026-09-17 | README.md | 顶部加 fork 声明与壳的安装命令，其余为上游原文 |
| 2026-09-17 | tests/cli/sessions.rs | `integration_commands_run_locally_when_server_is_missing` 改为断言壳的行为：install / uninstall 返回 2 且不写、不删文件，status 照常可用（`tests/cli` 只在非 macOS 的 unix 上编译，v0 时在 macOS 上没跑到） |
| 2026-09-17 | docs/next/website/src/data/config-reference.json | 补登记 `keys.toggle_composer`（配置参考与配置模型的一致性检查要求） |

## 同步上游

```
git fetch upstream --tags
git merge upstream/<tag>     # 在 ke/main 上合并，冲突时优先保留上游行为
```
`upstream` 的推送地址已设为 DISABLED，防止误推到上游仓库。
