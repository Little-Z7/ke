# 壳（ke）

本仓库是 [herdr](https://github.com/herdrdev/herdr) 的修改版（fork），基于 herdr v0.9.1（首个基线 v0.9.0，提交 b99002ac；2026-09-17 合并 v0.9.1），遵循 Apache License 2.0（见 LICENSE）。

- 产品名"壳"，安装后的命令名 `ke`，直接运行 `ke` 启动 TUI。与上游 herdr 不是同一产品，不隶属于 herdr 项目，也未获其背书；提到 herdr 只为说明来源。
- cargo 的二进制目标名仍是上游的 `herdr`，因为上游测试与工具依赖 `CARGO_BIN_EXE_herdr`；改名只发生在安装与打包这一步。不要用 `cargo install`，它会装出一个与上游同名的 `herdr`。子命令帮助里的 `herdr <command>` 同样保留上游写法（顶层帮助有说明），以便合并上游。
- 目标：让一个常驻模型（本地或云端）住在终端工作区里，协调各家 coding agent CLI；在 agent 窗格下提供原生输入栏（用户输入先经本机处理，如脱敏，再送入 CLI）；在侧栏显示各 CLI 的额度、任务与常驻模型状态。
- 与已安装的上游 herdr 完全隔离：独立的配置目录、socket 与会话文件，关闭自动更新。窗格内仍注入 `HERDR_ENV` / `HERDR_SOCKET_PATH` / `HERDR_PANE_ID`（指向壳自己的 socket），以兼容各 CLI 已安装的 herdr 状态钩子。

## 安装

macOS 与 Linux（x86_64 / aarch64）：

```
curl -fsSL https://github.com/Little-Z7/ke/releases/latest/download/ke-install.sh | sh
```

Windows（x86_64）：

```
powershell -ExecutionPolicy Bypass -c "irm https://github.com/Little-Z7/ke/releases/latest/download/ke-install.ps1 | iex"
```

- Unix 装到 `~/.local/bin/ke`（`KE_INSTALL_DIR` 可改），Windows 装到 `%LOCALAPPDATA%\Programs\ke\ke.exe`（含 ConPTY）。下载后按发布里的 `SHA256SUMS` 校验。`KE_RELEASE_TAG=ke-v0.1.0` 可指定版本。更新就是重新运行一次。
- 壳不自带 agent 状态钩子的安装：它复用上游 herdr 为各 CLI 装好的钩子（钩子按窗格里的 `HERDR_SOCKET_PATH` 找 socket，在壳的窗格里就会连到壳）。需要钩子的话先装上游 herdr 并执行 `herdr integration install <agent>`；没有钩子时壳退回到基于屏幕内容的状态检测。
- 输入栏的本机处理进程（脱敏等）和侧栏面板的协调进程不在本仓库里。没有它们时输入栏原文直通，侧栏 ke 面板不显示。两者的路径默认在 `~/.workcat/ke/` 下，可用 `config.toml` 的 `[ke]` 段（`composer_socket`、`panel_file`）或环境变量（`KE_COMPOSER_SOCKET`、`KE_PANEL_FILE` / `WORKCAT_KE_HOME`）修改。协议与用法见 `docs/next/website/src/content/docs/ke.mdx`（中文版 `zh-cn/ke.mdx`）。
- 从源码安装：Unix 用 `scripts/ke-install.sh`（需要 Rust 与 `vendor/libghostty-vt/build.zig.zon` 里 `minimum_zig_version` 指定的 Zig，herdr 0.9.1 起为 0.16.0）。Windows 用上面的 `ke-install.ps1`，或 `cargo build --release` 再跑 `scripts/package_windows_conpty.ps1`。

## 发布

版本号在 `src/build_info.rs` 的 `KE_VERSION`，与上游版本（`Cargo.toml`）分开。发布步骤：改 `KE_VERSION` 并提交，然后

```
git tag ke-v<KE_VERSION> && git push origin ke/main ke-v<KE_VERSION>
```

`.github/workflows/ke-release.yml` 会校验标签与 `KE_VERSION` 一致，构建五个平台的二进制（含 Windows zip + ConPTY），连同 `ke-install.sh`、`ke-install.ps1`、`SHA256SUMS`、`LICENSE` 发到 GitHub Release。上游自带的 workflow 只在 `master` 分支和 `v*` 标签上动作，所以壳用 `ke/main` 分支和 `ke-v*` 标签，不要推 `master` 分支或 `v*` 标签到壳的仓库。

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
| 2026-09-17 | .github/dependabot.yml（删除） | 壳跟随上游的依赖版本，不单独升级；上游的 Dependabot 配置在壳的仓库里只会开无关的 PR 并触发上游 CI |
| 2026-09-17 | src/config/model.rs | 新增 `[ke]` 配置段（`KeConfig`：`composer_socket`、`panel_file`）与路径解析（环境变量 > 配置 > 默认 `~/.workcat/ke/`，`~` 展开）；`Config` 加 `ke` 字段；3 个测试 |
| 2026-09-17 | src/config/io.rs、src/config.rs | `ke` 加入已知顶层段并按 live section 加载；导出 `KeConfig` |
| 2026-09-17 | src/client/shell/state.rs | `ClientShellConfig` 加 `ke`；面板数据源按配置路径构造；命中表加 `ke_panel_rows` |
| 2026-09-17 | src/client/shell/config.rs | `from_config` / `apply_live_config` 带上 `[ke]`；配置重载时重新指向面板文件 |
| 2026-09-17 | src/client/shell/composer.rs | 处理进程 socket 改为每次提交时按 `[ke] composer_socket` 解析并随请求传入钩子；新增 `composer_open_with`（面板点击打开输入栏并填入文字） |
| 2026-09-17 | src/client/shell/ke_panel.rs | 面板行可选 `input` 字段（控制字符替换、2000 字符上限、空白丢弃）；带 `input` 的行加下划线并返回点击区域；数据源改为传入路径、支持重载后改路径、路径为空时隐藏已显示的面板；4 个测试更新/新增 |
| 2026-09-17 | src/client/shell/sidebar.rs、src/client/shell/endpoint_sidebar.rs、src/client/shell/render.rs | 渲染面板时把点击区域写入命中表；关闭鼠标捕获时清空 |
| 2026-09-17 | src/client/shell/mouse.rs | 左键点到带 `input` 的面板行：打开输入栏并填入文字，不发送 |
| 2026-09-17 | src/client/shell/tests/composer_bar.rs | 新增面板点击测试 |
| 2026-09-17 | src/main.rs | 默认配置模板加注释掉的 `[ke]` 段 |
| 2026-09-17 | docs/next/website/src/data/config-reference.json | 新增 `ke` 段：`ke.composer_socket`、`ke.panel_file` |
| 2026-09-17 | docs/next/website/src/content/docs/ke.mdx、zh-cn/ke.mdx、ja/ke.mdx（新增） | 壳的用户文档：安装、与 herdr 的隔离、输入栏与处理进程协议、侧栏面板与 panel.json 格式、`[ke]` 配置、限制 |
| 2026-09-17 | （合并上游 v0.9.1） | 6 个文件 7 处冲突：`src/client/mod.rs`、`src/client/shell.rs`、`src/client/shell/state.rs`、`src/client/shell/composition.rs`（两侧都留）；`src/client/shell/composition.rs` 里 `ke_panel` 一行放进上游重构后的 `ShellRenderState`；`tests/cli/sessions.rs` 保留壳的"pi: not installed"断言 |
| 2026-09-17 | src/ke_env.rs（新增）、src/main.rs | 启动时清理继承的 `HERDR_*` 的逻辑从 `main.rs` 搬到独立文件，`main.rs` 只剩 `mod ke_env;` 与一行调用，减少与上游的位置冲突；补 2 个测试 |
| 2026-09-17 | src/client/shell.rs | 壳的 `mod composer; mod ke_panel;` 收到 mod 列表末尾，上游在列表中间加模块时不再冲突 |
| 2026-09-17 | scripts/ke-install.sh | Zig 回退路径按 `build.zig.zon` 的 `minimum_zig_version` 优先挑对应版本（herdr 0.9.1 起需要 Zig 0.16.0） |
| 2026-09-17 | .github/workflows/ke-release.yml | 与上游 release.yml 一致改用 `vercel-labs/setup-zig` 安装 Zig 0.16.0（含 macOS，去掉 Homebrew zig@0.15 变通） |
| 2026-09-17 | tests/machine_api.rs、tests/session_delete.rs | 上游 v0.9.1 新增的测试里应用目录名 herdr-dev → ke-dev（同 2026-09-13 对 tests/*.rs 的处理） |
| 2026-09-18 | src/ke/{mod,paths,redact,chat_log}.rs、src/ke/resident/{mod,panel,processor,supervisor}.rs（新增） | 管家 M1：`ke resident`、会话 `resident/` 目录、规则脱敏、composer 处理进程、确定性面板、服务端守护 |
| 2026-09-18 | src/cli.rs、src/cli/spec.rs、src/main.rs、src/server/headless/bootstrap.rs、src/app/mod.rs、src/config/model.rs、docs/next/website/src/data/config-reference.json | 管家 M1 挂钩：`resident` 子命令、`[ke.model]`/`[ke.redact]`、服务端随配置 spawn |
| 2026-09-18 | src/ke/resident/{model,snapshot,prompt,answer}.rs、src/ke/defaults/resident.md（新增） | 管家 M2：OpenAI 兼容 curl/SSE、窗格快照、角色宪法、`@ke` 异步回答写入 chat.jsonl |
| 2026-09-18 | src/ke/resident/mod.rs、src/ke/resident/processor.rs、src/config/model.rs、src/config.rs | 管家 M2：answerer 线程；`@ke` done note「思考中…」；`resolved_profile` 默认本机 Ollama |
| 2026-09-18 | src/client/shell/ke_chat.rs、src/client/shell/tests/ke_chat.rs（新增） | 管家 M2：客户端按 mtime 轮询 chat.jsonl，新 assistant 弹可滚动浮层（启动不刷历史） |
| 2026-09-18 | src/client/shell.rs、state.rs、overlays.rs、overlay_input.rs、composition.rs、mouse.rs、input.rs、config.rs、src/client/mod.rs、src/client/shell/tests/{mod,graphics}.rs | 管家 M2：接入 KeChat overlay（Esc/Enter/点击关闭，滚轮与方向键滚动；不改 protocol） |
| 2026-09-18 | src/config/model.rs、src/ke/chat_log.rs | `KeConfig::chat_log_path()`；`read_tail` 供客户端使用 |
| 2026-09-18 | src/ke/slash.rs、src/client/shell/ke_cmd.rs（新增） | 管家 M3：`/ke` 命令（help/status/log/model/provider/prompt/memory）与 `//` 逃逸，客户端本地执行 |
| 2026-09-18 | src/client/shell/composer.rs、src/client/shell/ke_chat.rs、src/client/shell.rs、src/ke/mod.rs | 管家 M3：提交路径拦截 `/ke` 与 `//`；`/ke log` 展开底栏记录 |
| 2026-09-18 | src/input/keybindings.rs、src/config/keybinds.rs、src/config/model.rs、src/input/keybind_help.rs、src/client/shell/actions.rs、src/main.rs、docs/next/website/src/data/config-reference.json | 管家 M3：`keys.ke_chat`（默认 prefix+shift+i）预填 `@ke ` |
| 2026-09-18 | src/client/shell/tests/composer_bar.rs | 管家 M3：`/ke help`、`//` 逃逸、其它斜杠命令仍进窗格、ke_chat 快捷键 |
| 2026-09-18 | src/ke/resident/processor.rs、src/client/shell/composer.rs、src/ke/resident/supervisor.rs、src/ke/resident/mod.rs | Windows：composer/resident 改走 `ipc::` 本地 socket（命名管道）；去掉 unix-only 门；面板原子写兼容 Windows rename |
| 2026-09-18 | .github/workflows/ke-release.yml、distribution/ke-install.ps1（新增）、src/build_info.rs | Windows 发布 `ke-windows-x86_64.zip`（含 ConPTY）；PowerShell 安装脚本；`KE_INSTALL_COMMAND` 按平台切换 |
| 2026-09-18 | docs/next/.../ke.mdx（en/zh-cn/ja）、FORK.md、README.md、config-reference.json | 文档与安装说明补 Windows |
| 2026-09-18 | src/client/shell/composer.rs、text_editor.rs、src/ke/slash.rs | 底栏长文本折行；`//`/`/ke` 指令列表（↑↓/Tab） |
| 2026-09-18 | src/client/shell/ke_model.rs（新增）、overlays.rs、overlay_input.rs、src/config/model.rs、src/ke/resident/model.rs | `/ke model` 打开模型配置 TUI；profile 增加 think / max_tokens |
| 2026-09-18 | src/ke/slash.rs、src/client/shell/ke_model.rs、ke_cmd.rs、overlays.rs | 模型弹层接入模板：Ollama / 火山方舟 Coding Plan / OpenAI；`/ke model ark` 可套用未写入的模板 |
| 2026-09-18 | src/ke/slash.rs、src/client/shell/ke_cmd.rs | `/ke update` 在底栏显示安装命令（与 CLI `ke update` 一致，不自更新） |
| 2026-09-18 | src/update.rs、src/main.rs、src/build_info.rs | 终端 `ke update` 跑 ke 官方安装脚本；不再只打印命令；仍禁用 herdr.dev 自更新 |
| 2026-09-18 | src/client/shell/state.rs、composer.rs | 桌面壳栏改到窗格右侧；窄屏仍用底栏 |
| 2026-09-18 | src/build_info.rs、src/main.rs、src/cli/spec.rs | 发布 ke-v0.3.1：`ke version` 等同 `--version`；安装包含右侧栏与会真正执行安装脚本的 `ke update` |
| 2026-09-18 | src/client/shell/composer.rs、mouse.rs、preferences.rs | 右侧壳栏可拖左侧 `│` 改宽，双击恢复 32 列，宽度写入 client-shell 偏好 |
| 2026-09-18 | src/client/shell/settings.rs、ke_model.rs、global_menu.rs | 模型配置并入 Settings 的 model 分页；`/ke model` 与全局菜单 model 打开同一套表单 |
| 2026-09-18 | src/build_info.rs | 发布 ke-v0.3.2：Settings 模型页与可拖宽壳栏 |
| 2026-09-20 | src/client/shell/composer.rs | 壳栏聚焦时 Ctrl+B 不再当光标左移，留给前缀键 |
| 2026-09-20 | CLAUDE.md | 由指向 AGENTS.md 的符号链接改为壳自己的文件：壳的定位、命令、ke 层结构、fork 纪律与发布流程；上游规则仍指向未改动的 AGENTS.md |
| 2026-09-20 | src/ke/resident/sessions.rs（新增）、mod.rs、panel.rs、snapshot.rs | 管家跨会话：枚举全部运行中会话并合并 roster；面板按 (会话, pane_id) 计时、非本会话行加 `[名字]` 前缀、连不上的会话单独一行；只有本会话 agent.list 失败才计入退出；窗格内容仍只读本会话，快照注明其它会话只有状态 |
| 2026-09-20 | src/ke/redact.rs | 可逆脱敏：`Mapping`（仅内存，不序列化）+ `mask`（按类型发放稳定占位符 `KE_SECRET_n`/`KE_IP_n`/`KE_EMAIL_n`，同一真实值跨调用始终同号）+ `restore`（占位符按长度倒序匹配，还原原文）；以完整占位符开头的值一律跳过，保证已脱敏文本二次 mask 是 no-op；现有 `redact()` 未改动，暂无调用点（带 `#[allow(dead_code)]` 与移除条件） |
| 2026-09-20 | src/ke/redact.rs、src/ke/resident/{mod,processor,answer}.rs | 脱敏路径改用可逆占位符：`Shared.redaction` 持会话级 `Mapping`，composer 提交与模型上下文（快照/用户消息/历史）四处共用同一份映射，锁只在 mask 期间持有；映射上限 10000 条，超限回退 `[REDACTED]`（不可逆但绝不放行明文），锁中毒同样回退；resident 启动时流式扫描 chat.jsonl 取各类占位符编号水位线，避免重启后复用编号导致两个真实值共号 |
| 2026-09-20 | src/ke/resident/model.rs、answer.rs | 实现 `provider = "cli"`：管家大脑可跑在现成 agent CLI 上（argv 取 `[ke.model.profiles.*].command`，prompt 拍平后走 stdin，读 stdout）；cwd 固定为 resident 目录，避免 CLI 读到用户项目的 AGENTS.md；三线程各管阻塞 IO、主线程只 try_wait 轮询，300 秒超时；unix 下子进程独立进程组，超时按组 SIGKILL，否则 shell 包装 fork 出的孙进程会握着管道让 reader 线程等到自然结束；输出只剥 ANSI 不猜测特定 CLI 的装饰行 |

## 同步上游

```
git fetch upstream --tags
git merge upstream/<tag>     # 在 ke/main 上合并，冲突时优先保留上游行为
```
`upstream` 的推送地址已设为 DISABLED，防止误推到上游仓库。
