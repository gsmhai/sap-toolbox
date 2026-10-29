# SAP 工具箱 (SAPToolbox)

基于 **Tauri 2 + Vue 3** SAP GUI 自动化工具。直接通过 Rust 调用 SAP GUI Scripting COM 接口。

## 功能

| 功能区 | 事务码 | 说明 |
| --- | --- | --- |
| ① 客户信用信息查询 | FD33 | 查询客户信用总额、未清订单、未清交货、应收总额、可用额度 |
| ② 订单金额批量查询 | VA03 | 批量查询订单抬头净值（可选进入条件页取 Total amount） |
| ③ 修改外向交货单输出 | VL02N | 修改/新增外向交货单的 ZPZ3 输出（Communication 设为 MAIL） |

## 界面

- 顶部：SAP 会话选择 + 刷新会话 + **隐藏日志**按钮
- 左侧：三个功能区
- 右侧：运行日志面板（可通过"隐藏日志"按钮关闭/打开）

## 运行日志

日志同时写入 Windows 用户 AppData 目录：

```
%APPDATA%\SAPToolbox\logs\sap-toolbox-YYYY-MM-DD.log
```

## 前置条件

- 目标电脑已安装 **SAP GUI**（SAP Logon），且至少登录了一个会话
- SAP GUI 选项 -> 辅助功能与脚本 -> 脚本 -> 已启用（允许脚本）

## 开发

```bash
# 安装依赖
npm install

# 本地开发（需要 Rust 工具链）
npm run tauri dev

# 本地构建
npm run tauri build
```

## 打包（免安装单 exe）

通过 GitHub Actions 手动触发构建：

1. 推送到 GitHub 仓库
2. 进入 **Actions** 页 -> 选择 **Build Windows EXE** -> **Run workflow**
3. 构建完成后，在 Release 和 Artifacts 中下载免安装单 exe

> 说明：已设置 `bundle.active = false`，构建时只产出绿色免安装单文件 `SAPToolbox-vX.X.X-portable.exe`，不生成 NSIS 安装包。目标机为 Win10/11（自带 WebView2 运行时）即可双击运行。

## 目录结构

```
sap-toolbox/
├── src/                      # Vue 3 前端
│   ├── App.vue               # 主界面
│   ├── main.ts
│   └── vite-env.d.ts
├── src-tauri/                # Rust 后端
│   ├── src/
│   │   ├── main.rs
│   │   ├── lib.rs            # Tauri 命令入口
│   │   ├── sap_com.rs        # SAP GUI COM 调用层
│   │   ├── sap_ops.rs        # 三个业务操作
│   │   └── logger.rs         # 日志封装
│   ├── icons/icon.ico
│   ├── Cargo.toml
│   └── tauri.conf.json
├── .github/workflows/build-exe.yml
└── package.json
```
