//! Tauri 应用入口：暴露 SAP 操作命令给前端。

mod logger;
mod sap_com;
mod sap_ops;

use std::sync::Mutex;

use tauri::{AppHandle, Manager, State};

use sap_com::{cleanup_com, connect_sap_gui, get_session, list_sessions, wait_sap, Session};
use sap_ops::{
    change_out_bound_delivery, get_customer_credit_info, get_delivery_output_info, get_order_amount,
    get_order_amounts,
};

/// 全局状态：记录当前是否正在执行操作（防止并发操作 SAP 会话）
struct BusyState(Mutex<bool>);

impl BusyState {
    fn new() -> Self {
        BusyState(Mutex::new(false))
    }
    fn try_lock(&self) -> bool {
        let mut guard = self.0.lock().unwrap();
        if *guard {
            false
        } else {
            *guard = true;
            true
        }
    }
    fn unlock(&self) {
        let mut guard = self.0.lock().unwrap();
        *guard = false;
    }
}

// ---------- 命令 ----------

/// 刷新 SAP 会话列表
#[tauri::command]
fn refresh_sessions(app: AppHandle) -> Result<Vec<(i32, i32, String)>, String> {
    logger::info(&app, "刷新 SAP 会话 ...");
    let app_disp = connect_sap_gui().map_err(|e| {
        logger::error(&app, &format!("读取 SAP 会话失败: {e}"));
        e
    })?;
    let sessions = list_sessions(&app_disp).map_err(|e| {
        logger::error(&app, &format!("读取 SAP 会话失败: {e}"));
        e
    })?;
    cleanup_com();
    logger::info(&app, &format!("SAP 会话刷新完成，共 {} 个会话", sessions.len()));
    Ok(sessions)
}

/// 客户信用信息查询
#[tauri::command]
fn query_credit(app: AppHandle, state: State<'_, BusyState>, customer_code: String, conn_idx: i32, sess_idx: i32) -> Result<sap_ops::CreditInfo, String> {
    if !state.try_lock() {
        return Err("上一个操作还在执行，请等待完成".to_string());
    }
    let result = run_with_session(&app, conn_idx, sess_idx, |session| {
        get_customer_credit_info(&app, session, &customer_code)
    });
    state.unlock();
    result
}

/// 订单金额批量查询（单个订单）
#[tauri::command]
fn query_order_amount(app: AppHandle, state: State<'_, BusyState>, order_no: String, only_head: bool, is_last: bool, conn_idx: i32, sess_idx: i32) -> Result<f64, String> {
    if !state.try_lock() {
        return Err("上一个操作还在执行，请等待完成".to_string());
    }
    let result = run_with_session(&app, conn_idx, sess_idx, |session| {
        get_order_amount(&app, session, &order_no, only_head, is_last)
    });
    state.unlock();
    result
}

/// 订单金额批量查询（只进入一次 VA03，在事务内逐个切换订单号）
///
/// 必须走 async + spawn_blocking：同步命令会阻塞 WebView 主线程，后端每单 emit 的
/// `order-amount://progress` 事件只能堆到整批结束才被处理，前端依然无法逐单刷新。
/// BusyState 改为从 AppHandle 取，避开 `State<'_, _>` 参数跨 await 的生命周期限制。
#[tauri::command]
async fn query_order_amounts(
    app: AppHandle,
    order_nos: Vec<String>,
    only_head: bool,
    conn_idx: i32,
    sess_idx: i32,
) -> Result<Vec<(String, f64, String)>, String> {
    let state = app.state::<BusyState>();
    if !state.try_lock() {
        return Err("上一个操作还在执行，请等待完成".to_string());
    }
    let task_app = app.clone();
    let joined = tauri::async_runtime::spawn_blocking(move || {
        run_with_session(&task_app, conn_idx, sess_idx, |session| {
            get_order_amounts(&task_app, session, &order_nos, only_head)
        })
    })
    .await;
    state.unlock();
    match joined {
        Ok(result) => result,
        Err(e) => {
            logger::error(&app, &format!("批量查询任务异常结束: {e}"));
            Err(format!("任务执行失败: {e}"))
        }
    }
}

/// 查询交货单输出信息（料号 + 描述）
#[tauri::command]
fn query_delivery_output(app: AppHandle, state: State<'_, BusyState>, order_no: String, conn_idx: i32, sess_idx: i32) -> Result<Vec<(String, String)>, String> {
    if !state.try_lock() {
        return Err("上一个操作还在执行，请等待完成".to_string());
    }
    let result = run_with_session(&app, conn_idx, sess_idx, |session| {
        get_delivery_output_info(&app, session, &order_no)
    });
    state.unlock();
    result
}

/// 修改外向交货单（每次处理一行物料）
#[tauri::command]
fn modify_delivery(app: AppHandle, state: State<'_, BusyState>, order_no: String, is_create: bool, lang: String, is_save: bool, row_index: i32, conn_idx: i32, sess_idx: i32) -> Result<String, String> {
    if !state.try_lock() {
        return Err("上一个操作还在执行，请等待完成".to_string());
    }
    let result = run_with_session(&app, conn_idx, sess_idx, |session| {
        change_out_bound_delivery(&app, session, &order_no, is_create, &lang, is_save, row_index)
    });
    state.unlock();
    result
}

/// 获取日志目录（供前端展示）
#[tauri::command]
fn get_log_dir() -> String {
    logger::log_dir_string()
}

/// 导出 CSV（内容由前端组装，后端用标准库写文件，避免 fs 插件作用域限制）
#[tauri::command]
fn export_csv(app: AppHandle, path: String, content: String) -> Result<(), String> {
    std::fs::write(&path, content).map_err(|e| {
        let msg = format!("导出 CSV 失败: {e}");
        logger::error(&app, &msg);
        msg
    })?;
    logger::info(&app, &format!("已导出: {path}"));
    Ok(())
}

/// 在独立线程中获取 SAP 会话并执行操作（每个线程自行 CoInitialize）
fn run_with_session<T, F>(
    app: &AppHandle,
    conn_idx: i32,
    sess_idx: i32,
    f: F,
) -> Result<T, String>
where
    F: FnOnce(&Session) -> Result<T, String>,
{
    let app_disp = connect_sap_gui().map_err(|e| {
        logger::error(app, &format!("连接 SAP GUI 失败: {e}"));
        e
    })?;
    let sess_disp = get_session(&app_disp, conn_idx, sess_idx).map_err(|e| {
        logger::error(app, &format!("获取会话失败: {e}"));
        e
    })?;
    let session = Session::new(sess_disp);
    let _ = wait_sap(&session, 0.5); // 初始等待
    let result = f(&session);
    cleanup_com();
    result
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(BusyState::new())
        .invoke_handler(tauri::generate_handler![
            refresh_sessions,
            query_credit,
            query_order_amount,
            query_order_amounts,
            query_delivery_output,
            modify_delivery,
            get_log_dir,
            export_csv,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
