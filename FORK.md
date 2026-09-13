# 壳（ke）

本仓库是 [herdr](https://github.com/herdrdev/herdr) 的修改版（fork），基于 herdr v0.9.0（提交 b99002ac），遵循 Apache License 2.0（见 LICENSE）。

- 产品名"壳"，二进制名 `ke`。与上游 herdr 不是同一产品，不使用 herdr 的名称或商标发布。
- 目标：让一个常驻模型（本地或云端）住在终端工作区里，协调各家 coding agent CLI；在 agent 窗格下提供原生输入栏（用户输入先经本机处理，如脱敏，再送入 CLI）；在侧栏显示各 CLI 的额度、任务与常驻模型状态。
- 与已安装的上游 herdr 完全隔离：独立的配置目录、socket 与会话文件，关闭自动更新。窗格内仍注入 `HERDR_ENV` / `HERDR_SOCKET_PATH` / `HERDR_PANE_ID`（指向壳自己的 socket），以兼容各 CLI 已安装的 herdr 状态钩子。

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

## 同步上游

```
git fetch upstream --tags
git merge upstream/<tag>     # 在 ke/main 上合并，冲突时优先保留上游行为
```
`upstream` 的推送地址已设为 DISABLED，防止误推到上游仓库。
