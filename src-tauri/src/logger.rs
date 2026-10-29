//! 日志模块：统一封装运行日志。
//! - 通过 Tauri 事件 `log://entry` 推送到前端右侧日志面板
//! - 同时写入 Windows 用户 AppData 目录下的日志文件
//!   `%APPDATA%/SAPToolbox/logs/sap-toolbox-YYYY-MM-DD.log`

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::Local;
use tauri::{AppHandle, Emitter};

/// 全局日志文件句柄（延迟初始化）
static LOG_FILE: Mutex<Option<std::fs::File>> = Mutex::new(None);

/// 计算日志目录：%APPDATA%/SAPToolbox/logs
fn log_dir() -> PathBuf {
    let mut dir = appdata_dir();
    dir.push("SAPToolbox");
    dir.push("logs");
    dir
}

/// 获取 Windows 用户 AppData/Roaming 目录
/// Windows 官方约定：环境变量 APPDATA = C:\Users\<用户名>\AppData\Roaming
fn appdata_dir() -> PathBuf {
    if let Ok(appdata) = std::env::var("APPDATA") {
        if !appdata.is_empty() {
            return PathBuf::from(appdata);
        }
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        return PathBuf::from(home).join("AppData").join("Roaming");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home);
    }
    PathBuf::from(".")
}

/// 打开（或复用）日志文件，按天命名
fn ensure_log_file() -> std::io::Result<std::fs::File> {
    let mut guard = LOG_FILE.lock().unwrap();
    if let Some(f) = guard.as_ref() {
        return f.try_clone();
    }
    let dir = log_dir();
    fs::create_dir_all(&dir)?;
    let today = Local::now().format("%Y-%m-%d").to_string();
    let path = dir.join(format!("sap-toolbox-{}.log", today));
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    *guard = Some(file.try_clone()?);
    Ok(file)
}

/// 核心日志函数：同时写文件 + 推送前端
pub fn log(app: &AppHandle, level: &str, msg: &str) {
    let ts = Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string();
    let line = format!("[{}] [{}] {}", ts, level, msg);

    // 1. 写文件
    if let Ok(mut f) = ensure_log_file() {
        let _ = writeln!(f, "{}", line);
        let _ = f.flush();
    }

    // 2. 推送到前端（事件名 log://entry）
    let _ = app.emit("log://entry", serde_json::json!({
        "time": ts,
        "level": level,
        "message": msg,
    }));
}

/// 便捷封装
pub fn info(app: &AppHandle, msg: &str) {
    log(app, "INFO", msg);
}
pub fn warn(app: &AppHandle, msg: &str) {
    log(app, "WARN", msg);
}
pub fn error(app: &AppHandle, msg: &str) {
    log(app, "ERROR", msg);
}

/// 返回当前日志目录路径（供前端展示）
pub fn log_dir_string() -> String {
    log_dir().to_string_lossy().to_string()
}
