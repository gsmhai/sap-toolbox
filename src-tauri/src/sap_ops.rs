//! 三个 SAP 业务操作，等价于原 test.py 中的函数：
//!   GetCustomerCreditInfo(session, CustomerCode)   -> FD33
//!   GetOrderAmount(session, OrderNo, onlyHead)     -> VA03
//!   ChangeOutBoundDelivery(session, OrderNo)       -> VL02N
//!
//! 每个操作接收 AppHandle，把进度信息输出到前端日志面板 + 日志文件。

use tauri::{AppHandle, Emitter};

use crate::logger;
use crate::sap_com::{bool_variant, i32_variant, parse_amount, str_variant, variant_to_disp_public, wait_sap, Disp, Session};

/// 客户信用信息查询结果
#[derive(serde::Serialize, Clone)]
pub struct CreditInfo {
    pub total_credit_limit: f64,
    pub open_orders: f64,
    pub open_delivery: f64,
    pub total_receivables: f64,
    pub available_delivery: f64,
}

// ---------- 常用元素 ID ----------
/// FD33 信用总额（客户不存在时读不到）
const CLALL_ID: &str = "wnd[0]/usr/tabsTABSTRIP_SEGMENTS/tabpTAB_1/ssub%_SUBSCREEN_SEGMENTS:ZF_FSCM_FD33_INFO_SHOW:1001/txtP_CLALL";
/// VA03 抬头"条件"按钮（非抬头净值模式下先探测它）
const BT_HEAD_ID: &str = "wnd[0]/usr/subSUBSCREEN_HEADER:SAPMV45A:4021/btnBT_HEAD";
/// SAP 状态栏
const SBAR_ID: &str = "wnd[0]/sbar";
/// SAP 信息弹窗文本
const MSG_TXT_ID: &str = "wnd[1]/usr/txtMESSTXT1";

/// 读取状态栏/弹窗提示（读不到就返回空串，不抛错）
fn read_status_text(session: &Session, id: &str) -> String {
    session.get_text(id).unwrap_or_default().trim().to_string()
}

/// 将 SAP 返回的日期 YYYY-MM-DD 与时间 HH:MM:SS 合并为 "YYYY-MM-DD HH:MM:SS"
/// （SAP 已是带分隔符格式，直接拼接；时间为空时仅返回日期）
fn fmt_sap_datetime(date: &str, time: &str) -> String {
    if time.is_empty() {
        date.to_string()
    } else {
        format!("{date} {time}")
    }
}

/// ① 客户信用信息查询 (FD33)
pub fn get_customer_credit_info(
    app: &AppHandle,
    session: &Session,
    customer_code: &str,
) -> Result<CreditInfo, String> {
    logger::info(app, &format!("FD33 查询客户信用: {customer_code} ..."));
    if customer_code.trim().is_empty() {
        return Err("客户编码为空".to_string());
    }
    session.maximize()?;
    session.start_transaction("FD33")?;
    wait_sap(session, 5.0)?;
    session.set_text("wnd[0]/usr/ctxtP_KUNBP", customer_code)?;
    session.send_vkey(0)?;
    wait_sap(session, 5.0)?;

    // 防护：客户编码非法（不存在/未被维护）时信用字段读不到，SAP 会弹窗提示
    let clall = match session.get_text(CLALL_ID) {
        Ok(v) => v,
        Err(e) => {
            let msg = read_status_text(session, MSG_TXT_ID);
            let low = msg.to_lowercase();
            let _ = session.send_vkey(0); // 关掉信息弹窗（若有）
            let _ = session.send_vkey(3); // BACK 退出 FD33（与 Python 一致）
            if low.contains("no customer") || low.contains("not in") {
                logger::warn(app, &format!("FD33 客户 {customer_code} 不存在: {msg}"));
                return Err(format!("客户 {customer_code} 不存在"));
            }
            logger::error(app, &format!("FD33 读取信用总额失败: {e}；提示: {msg}"));
            return Err(format!("读取信用数据失败: {e}"));
        }
    };
    let total_credit_limit = parse_amount(&clall);
    let open_orders = parse_amount(&session.get_text(
        "wnd[0]/usr/tabsTABSTRIP_SEGMENTS/tabpTAB_1/ssub%_SUBSCREEN_SEGMENTS:ZF_FSCM_FD33_INFO_SHOW:1001/txtP_OBORH",
    )?);
    let open_delivery = parse_amount(&session.get_text(
        "wnd[0]/usr/tabsTABSTRIP_SEGMENTS/tabpTAB_1/ssub%_SUBSCREEN_SEGMENTS:ZF_FSCM_FD33_INFO_SHOW:1001/txtP_OBDEH",
    )?);
    let total_receivables = parse_amount(&session.get_text(
        "wnd[0]/usr/tabsTABSTRIP_SEGMENTS/tabpTAB_1/ssub%_SUBSCREEN_SEGMENTS:ZF_FSCM_FD33_INFO_SHOW:1001/txtP_FOGEH",
    )?);
    let available_delivery = total_credit_limit - open_delivery - total_receivables;

    logger::info(
        app,
        &format!(
            "FD33 返回: {total_credit_limit}, {open_orders}, {open_delivery}, {total_receivables} -> 可用 {available_delivery}"
        ),
    );

    session.send_vkey(3)?; // BACK
    Ok(CreditInfo {
        total_credit_limit,
        open_orders,
        open_delivery,
        total_receivables,
        available_delivery,
    })
}

/// ② 订单金额查询 (VA03)
pub fn get_order_amount(
    app: &AppHandle,
    session: &Session,
    order_no: &str,
    only_head: bool,
    is_last: bool,
) -> Result<f64, String> {
    logger::info(app, &format!("VA03 查询订单金额: {order_no}"));
    if order_no.trim().is_empty() {
        return Err("订单号为空".to_string());
    }
    session.maximize()?;
    session.start_transaction("VA03")?;
    wait_sap(session, 5.0)?;
    session.set_text("wnd[0]/usr/ctxtVBAK-VBELN", order_no)?;
    session.send_vkey(0)?;
    wait_sap(session, 5.0)?;
    session.send_vkey(0)?;
    wait_sap(session, 5.0)?;

    // 防护：订单号非法时目标字段不存在，SAP 会在状态栏提示
    let amount: f64 = match ensure_order_readable(app, session, order_no, only_head) {
        Ok(()) => {
            if only_head {
                parse_amount(&session.get_text(NETWR_ID)?)
            } else {
                read_price_total(app, session)?
            }
        }
        Err(e) => {
            let _ = session.send_vkey(3); // 尝试退出当前界面
            return Err(e);
        }
    };

    logger::info(app, &format!("订单 {order_no} 金额: {amount}"));

    // 收尾导航（计入本函数耗时）：仅最后一条订单（is_last=true）退出 VA03，
    // 与 Python GetOrderAmount 的 isLast 一致：退回订单号输入页 -> 退出 VA03；
    // 其余订单不额外 BACK，由下一次 start_transaction 重新进入 VA03。
    // 收尾失败不影响已读到的金额，故忽略单次 BACK 错误。
    if is_last {
        let _ = session.send_vkey(3); // BACK：退回 VA03 初始输入界面
        let _ = session.send_vkey(3); // BACK：退出 VA03
    }

    Ok(amount)
}

/// 探测 VA03 当前界面是否能读到金额字段。
/// 订单号非法（不存在/已归档）时字段不存在，SAP 会在状态栏给出提示，这里转成可读的错误。
fn ensure_order_readable(
    app: &AppHandle,
    session: &Session,
    order_no: &str,
    only_head: bool,
) -> Result<(), String> {
    let probe_id = if only_head { NETWR_ID } else { BT_HEAD_ID };
    if session.find_by_id(probe_id).is_ok() {
        return Ok(());
    }

    let status = read_status_text(session, SBAR_ID);
    let low = status.to_lowercase();
    // 与 Python GetOrderAmount 一致：状态栏含 "not in" 即视为订单不存在/已归档
    if low.contains("not in") {
        logger::warn(app, &format!("订单 {order_no} 不存在或已归档（状态栏: {status}）"));
        return Err(format!("订单 {order_no} 不存在或已归档"));
    }
    logger::error(app, &format!("订单 {order_no} 界面读取失败（状态栏: {status}）"));
    Err(format!(
        "界面读取失败（状态栏: {}）",
        if status.is_empty() { "无提示" } else { &status }
    ))
}

/// 打开定价页签并读取 "Total amount CRM" 行的条件值（读到后按一次 BACK 退出页签）
/// 行定位逻辑与 Python GetOrderAmount / GetOrderAmounts 的定价分支完全一致：
/// 用相对行号 row 计数（滚动后归零），命中即停；未命中时退回最后一行兜底。
fn read_price_total(app: &AppHandle, session: &Session) -> Result<f64, String> {
    session
        .find_by_id("wnd[0]/usr/subSUBSCREEN_HEADER:SAPMV45A:4021/btnBT_HEAD")?
        .call("press", vec![])?;
    wait_sap(session, 5.0)?;
    session
        .find_by_id(r"wnd[0]/usr/tabsTAXI_TABSTRIP_HEAD/tabpT\05")?
        .call("select", vec![])?;

    let mut price_table = session.find_by_id(PRICE_TABLE_ID)?;

    let columns = price_table.get_disp("Columns")?;
    let col_count = collection_count(&columns);
    let mut col: i32 = 0;
    let mut desc_col: i32 = 0;
    for i in 0..col_count {
        let column = columns.item(i)?;
        let title = column.get_str("Title").unwrap_or_default();
        if title == "Description" {
            desc_col = i;
        } else if title == "Condition Value" {
            col = i;
            break;
        }
    }

    // 在定价网格中定位 "Total amount CRM" 行（与 Python GetOrderAmount / GetOrderAmounts 完全对齐）。
    // 网格行数可能超过视口：每轮先判断是否需要滚动，把目标行滚进可见区后再读。
    // 滚动后按滚动条实际 position 计算相对行（正常不夹紧时为 0），夹紧时自动修正并记警告。
    let max_row = price_table.get_i32("RowCount").unwrap_or(0);
    let mut visible_row = price_table
        .get_i32("VisibleRowCount")
        .unwrap_or(max_row.max(1));
    let mut row: i32 = 0;
    let mut found = false;
    for idx in 0..max_row {
        // 先判断是否需要滚动，再读当前行（与 Python GetOrderAmount / GetOrderAmounts 一致）：
        // idx >= visible_row 时滚到 idx-1，滚动后 row 归零，读到的即为下一个绝对行。
        if idx >= visible_row {
            let scrollbar = price_table.get_disp("verticalScrollbar")?;
            scrollbar.set("position", i32_variant(idx - 1))?;
            wait_sap(session, 5.0)?;
            price_table = session.find_by_id(PRICE_TABLE_ID)?;
            visible_row += price_table.get_i32("VisibleRowCount").unwrap_or(0);
            // 回读滚动条实际位置（对应 Python 的 "Scroll to ... fail" 分支）：
            // 控件可能把 position 夹紧到可停上限，此时 row=0 与期望绝对行错位，
            // 用实际 pos 反算相对行，避免漏读或重复读某一行。
            let actual_pos = price_table
                .get_disp("verticalScrollbar")
                .and_then(|sb| sb.get_i32("position"))
                .unwrap_or(-1);
            if actual_pos != idx - 1 {
                logger::warn(
                    app,
                    &format!(
                        "滚动到 {} 失败，当前滚动条位置: {}（按实际位置修正相对行）",
                        idx - 1,
                        actual_pos
                    ),
                );
                row = idx - (actual_pos + 1);
            } else {
                row = 0;
            }
        }
        let desc = get_cell_text(&price_table, row, desc_col)?;
        if desc.contains("Total amount CRM") {
            found = true;
            break;
        } else {
            row += 1;
        }
    }
    // 行号命中保护：未找到 "Total amount CRM" 时退回最后一行兜底（与 Python isfound 标志一致）
    if !found {
        row -= 1;
        logger::warn(
            app,
            &format!("未在 {max_row} 行内找到 Total amount CRM，改用最后一行 {row}"),
        );
    }

    let amount = parse_amount(&get_cell_text(&price_table, row, col)?);
    session.send_vkey(3)?; // BACK（退出定价页签）
    Ok(amount)
}

/// VA03 订单号输入框（也是判断"是否回到初始界面"的标志）
const ORDER_ENTRY_ID: &str = "wnd[0]/usr/ctxtVBAK-VBELN";
/// VA03 抬头净值字段
const NETWR_ID: &str = "wnd[0]/usr/subSUBSCREEN_HEADER:SAPMV45A:4021/txtVBAK-NETWR";
/// VA03 定价页签金额表格 (SAPLV69A Grid)
const PRICE_TABLE_ID: &str =
    r"wnd[0]/usr/tabsTAXI_TABSTRIP_HEAD/tabpT\05/ssubSUBSCREEN_BODY:SAPLV69A:6201/tblSAPLV69ATCTRL_KONDITIONEN";

/// 确认已经回到 VA03 初始输入界面（找不到订单号输入框就再按 BACK，最多重试 3 次）
fn back_to_order_entry(app: &AppHandle, session: &Session) -> Result<(), String> {
    for attempt in 1..=3 {
        wait_sap(session, 2.0)?;
        if session.find_by_id(ORDER_ENTRY_ID).is_ok() {
            return Ok(());
        }
        logger::warn(app, &format!("未回到 VA03 初始界面，第 {attempt} 次 BACK"));
        session
            .send_vkey(3)
            .map_err(|e| format!("按 BACK 失败: {e}"))?;
    }
    Err("连按 3 次 BACK 后仍未回到 VA03 初始界面".to_string())
}

/// 向前端推送单条订单的查询进度（批量模式下让表格逐行刷新）
/// index 为订单在入参中的下标，前端按下标回填，避免重复订单号时定位错行。
fn emit_progress(app: &AppHandle, index: usize, order_no: &str, amount: f64, error: &str) {
    let _ = app.emit(
        "order-amount://progress",
        serde_json::json!({
            "index": index,
            "orderNo": order_no,
            "amount": amount,
            "error": error,
        }),
    );
}

/// ②b 订单金额批量查询：只进入一次 VA03，在事务内逐个切换订单号
/// 返回 (订单号, 金额, 错误信息)；错误信息为空表示成功。单个订单失败不会中断整批。
/// 每完成一单都会 emit `order-amount://progress`，前端据此逐行更新。
pub fn get_order_amounts(
    app: &AppHandle,
    session: &Session,
    order_nos: &[String],
    only_head: bool,
) -> Result<Vec<(String, f64, String)>, String> {
    logger::info(
        app,
        &format!(
            "VA03 批量查询订单金额: {} 个 (Head={only_head}) ...",
            order_nos.len()
        ),
    );
    session.maximize()?;
    session.start_transaction("VA03")?;
    wait_sap(session, 5.0)?;

    let mut results: Vec<(String, f64, String)> = Vec::new();
    for (idx, order_no) in order_nos.iter().enumerate() {
        // 除第一个外，先确认已经退回到 VA03 初始界面
        if idx > 0 {
            if let Err(e) = back_to_order_entry(app, session) {
                let msg = format!("准备下一个订单失败: {e}");
                results.push((order_no.clone(), 0.0, msg.clone()));
                emit_progress(app, idx, order_no, 0.0, &msg);
                continue;
            }
        }

        let one = || -> Result<f64, String> {
            if order_no.trim().is_empty() {
                return Err("订单号为空".to_string());
            }
            session
                .set_text(ORDER_ENTRY_ID, order_no)
                .map_err(|e| format!("设置订单号失败: {e}"))?;
            session
                .send_vkey(0)
                .map_err(|e| format!("首次回车失败: {e}"))?;
            wait_sap(session, 5.0)?;
            session
                .send_vkey(0)
                .map_err(|e| format!("第二次回车失败: {e}"))?;
            wait_sap(session, 5.0)?;
            ensure_order_readable(app, session, order_no, only_head)?;
            if only_head {
                let text = session
                    .get_text(NETWR_ID)
                    .map_err(|e| format!("读取抬头净值失败: {e}"))?;
                Ok(parse_amount(&text))
            } else {
                read_price_total(app, session).map_err(|e| format!("读取定价合计失败: {e}"))
            }
        };

        match one() {
            Ok(amount) => {
                logger::info(app, &format!("订单 {order_no} 金额: {amount}"));
                results.push((order_no.clone(), amount, String::new()));
                emit_progress(app, idx, order_no, amount, "");
            }
            Err(e) => {
                logger::error(app, &format!("订单 {order_no} 查询失败: {e}"));
                results.push((order_no.clone(), 0.0, e.clone()));
                emit_progress(app, idx, order_no, 0.0, &e);
            }
        }
        let _ = session.send_vkey(3);
    }
    let _ = session.send_vkey(3);
    Ok(results)
}

/// 读取集合 Count 属性（不抛错，失败返回 0）
fn collection_count(d: &Disp) -> i32 {
    d.get_i32("Count").map(|c| c.abs()).unwrap_or(0)
}

/// 读取 SAP Grid 表格单元格（返回 IDispatch）
fn get_cell_disp(table: &Disp, row: i32, col: i32) -> Result<Disp, String> {
    let v = table.call("GetCell", vec![i32_variant(row), i32_variant(col)])?;
    variant_to_disp_public(&v)
}

/// 读取 SAP Grid 表格单元格文本
fn get_cell_text(table: &Disp, row: i32, col: i32) -> Result<String, String> {
    let cell = get_cell_disp(table, row, col)?;
    cell.get_str("Text")
}

/// 读取单元格图标名（IconName）
fn get_cell_icon(table: &Disp, row: i32, col: i32) -> Result<String, String> {
    let cell = get_cell_disp(table, row, col)?;
    cell.get_str("IconName")
}

/// 设置单元格文本
fn set_cell_text(table: &Disp, row: i32, col: i32, value: &str) -> Result<(), String> {
    let cell = get_cell_disp(table, row, col)?;
    cell.set("Text", str_variant(value))
        .or_else(|_| cell.set("text", str_variant(value)))
}

/// 选中某行（Rows(i).selected = true）
fn select_row(table: &Disp, row: i32) -> Result<(), String> {
    let rows = table.get_disp("Rows")?;
    let r = rows.item(row)?;
    r.set("selected", bool_variant(true))
}

/// ③ 修改外向交货单输出 ZPZ3 (VL02N)
pub fn change_out_bound_delivery(
    app: &AppHandle,
    session: &Session,
    order_no: &str,
    is_save: bool,
) -> Result<Vec<String>, String> {
    logger::info(app, &format!("VL02N 修改外向交货单输出 ZPZ3: {order_no} (isSave={is_save}) ..."));
    if order_no.trim().is_empty() {
        return Ok(vec!["交货单号为空".to_string()]);
    }
    session.maximize()?;
    session.start_transaction("VL02N")?;
    wait_sap(session, 5.0)?;
    session.set_text("wnd[0]/usr/ctxtLIKP-VBELN", order_no)?;
    session.send_vkey(0)?;
    wait_sap(session, 5.0)?;

    let delivery_table_id = r"wnd[0]/usr/tabsTAXI_TABSTRIP_OVERVIEW/tabpT\01/ssubSUBSCREEN_BODY:SAPMV50A:1102/tblSAPMV50ATC_LIPS_OVER";
    let output_table_id = r"wnd[0]/usr/tblSAPDV70ATC_NAST3";

    let mut output_info: Vec<String> = Vec::new();
    // 防护：交货单号非法（不存在）时明细表格读不到，SAP 会在状态栏提示
    let mut delivery_table = match session.find_by_id(delivery_table_id) {
        Ok(t) => t,
        Err(e) => {
            let status = read_status_text(session, SBAR_ID);
            let low = status.to_lowercase();
            let _ = session.send_vkey(3); // BACK 退出 VL02N（与 Python 一致）
            // 与 Python ChangeOutBoundDelivery 一致：状态栏含 "not exist" 即视为交货单不存在
            if low.contains("not exist") {
                let msg = format!("{order_no} 不存在");
                logger::warn(app, &format!("VL02N {msg}（状态栏: {status}）"));
                return Ok(vec![msg]);
            }
            let msg = format!("{order_no} 获取表异常");
            logger::error(app, &format!("VL02N 读取交货单表格失败: {e}；状态栏: {status}"));
            return Ok(vec![msg]);
        }
    };

    let max_row = delivery_table.get_i32("RowCount").unwrap_or(0);
    let mut visible_row = delivery_table
        .get_i32("VisibleRowCount")
        .unwrap_or(max_row.max(1));
    let mut row: i32 = 0;

    for i in 0..max_row {
        // 每次迭代重新获取表格：上一轮末尾的 BACK / press / select 会重建界面，
        // 旧的 COM 引用会失效，再调用其 GetCell 会返回 E_NOTIMPL(0x80004001)。
        // test.py 也在每次循环末尾重新 findById(deliveryTableId)。
        delivery_table = session.find_by_id(delivery_table_id)?;

        // 先判断是否需要滚动（与 Python 一致：i >= visualRow 时滚到 i-1），
        // 再用滚动条实际 position 计算本行在视口中的相对位置（不夹紧时为 0）。
        if i >= visible_row {
            let scrollbar = delivery_table.get_disp("verticalScrollbar")?;
            scrollbar.set("position", i32_variant(i - 1))?;
            wait_sap(session, 5.0)?;
            delivery_table = session.find_by_id(delivery_table_id)?;
            visible_row += delivery_table.get_i32("VisibleRowCount").unwrap_or(0);
            // 回读实际位置（对应 Python 的 "Scroll to ... fail" 分支）：
            // 被夹紧时 row=0 会与期望绝对行错位（某行重复处理、另一行漏处理），
            // 用实际 pos 反算相对行。
            let actual_pos = delivery_table
                .get_disp("verticalScrollbar")
                .and_then(|sb| sb.get_i32("position"))
                .unwrap_or(-1);
            if actual_pos != i - 1 {
                logger::warn(
                    app,
                    &format!(
                        "滚动到 {} 失败，当前滚动条位置: {}（按实际位置修正相对行）",
                        i - 1,
                        actual_pos
                    ),
                );
                row = i - (actual_pos + 1);
            } else {
                row = 0;
            }
        }

        // 读取当前可见行；下面 select 用的是同一个 row（修复旧版"读 N 行、选 N+1 行"的错位）
        let material = get_cell_text(&delivery_table, row, 1)?;
        if material.is_empty() {
            logger::info(app, "All order set output");
            break;
        }
        select_row(&delivery_table, row)?;
        row += 1; // 选中后再前进到下一行

        // 菜单：输出
        session
            .find_by_id("wnd[0]/mbar/menu[3]/menu[2]/menu[1]")?
            .call("select", vec![])?;
        wait_sap(session, 5.0)?;

        let mut output_table = session.find_by_id(output_table_id)?;

        let mut zpz3_list: Vec<(String, String, String)> = Vec::new();
        let mut end_row: i32 = 0;
        let mut need_create = true;

        let visible_count = output_table
            .get_i32("VisibleRowCount")
            .map(|c| c.abs())
            .unwrap_or(0);

        for j in 0..visible_count {
            let status = get_cell_icon(&output_table, j, 0)?;
            let out_type = get_cell_text(&output_table, j, 1)?;
            let proce_date = get_cell_text(&output_table, j, 8)?;
            let proce_time = get_cell_text(&output_table, j, 9)?;

            if out_type == "ZPZ3" {
                zpz3_list.push((status.clone(), proce_date.clone(), proce_time.clone()));
            }
            if out_type == "ZPZ3" && status == "S_TL_Y" {
                logger::info(app, &format!("找到需修改的 ZPZ3，行: {j}"));
                need_create = false;
                select_row(&output_table, j)?;
                session.find_by_id("wnd[0]/tbar[1]/btn[2]")?.call("press", vec![])?; // Communication
                wait_sap(session, 5.0)?;
                session.set_text("wnd[0]/usr/ctxtNAST-LDEST", "MAIL")?;
                session.find_by_id("wnd[0]/tbar[0]/btn[3]")?.call("press", vec![])?; // BACK
                wait_sap(session, 5.0)?;
                output_table = session.find_by_id(output_table_id)?;
                select_row(&output_table, j)?;
                session.find_by_id("wnd[0]/tbar[1]/btn[5]")?.call("press", vec![])?; // Further data
                wait_sap(session, 5.0)?;
                session.set_key("wnd[0]/usr/cmbNAST-VSZTP", "4")?;
                session.find_by_id("wnd[0]/tbar[0]/btn[3]")?.call("press", vec![])?; // BACK
                wait_sap(session, 5.0)?;
                if is_save {
                    session.find_by_id("wnd[0]/tbar[0]/btn[11]")?.call("press", vec![])?; // SAVE
                    output_info.push(format!("{material} {proce_date} 修改ZPZ3完成"));
                }
                else {
                    output_info.push(format!("{material} {proce_date} 修改ZPZ3完成(未保存)"));
                }
                break;
            }
            if out_type.is_empty() || status.is_empty() {
                end_row = j;
                logger::info(app, &format!("最后一行: {end_row}，最大可见行: {visible_count}"));
                break;
            }
        }

        logger::info(app, &format!("ZPZ3 列表: {:?}", zpz3_list));
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let today_date = chrono::Local::now().date_naive();
        let mut existing_time: Option<String> = None; // 保存已存在 ZPZ3 的完整时间(YYYY-MM-DD HH:MM:SS)，供日志打印
        for (status, pro_date, pro_time) in &zpz3_list {
            let within_days = match chrono::NaiveDate::parse_from_str(pro_date, "%Y-%m-%d") {
                Ok(d) => (today_date - d).num_days().abs() <= 3,
                Err(_) => {
                    logger::warn(app, &format!("{material} ZPZ3 处理日期无法解析: {pro_date}"));
                    false
                }
            };
            if status == "S_TL_G" && within_days {
                need_create = false;
                let combined = fmt_sap_datetime(pro_date, pro_time);
                existing_time = Some(combined.clone());
                logger::info(app, &format!("{material} 3天内已设置 ZPZ3，时间: {combined}"));
                break;
            }
        }

        if need_create {
            if let Err(e) = set_cell_text(&output_table, end_row, 1, "ZPZ3") {
                logger::error(app, &format!("{material} 设置 ZPZ3 失败: {e}"));
                output_info.push(format!("{material} 设置 ZPZ3 失败: {e}"));
                break;
            }
            select_row(&output_table, end_row)?;
            session.find_by_id("wnd[0]/tbar[1]/btn[2]")?.call("press", vec![])?; // Communication
            wait_sap(session, 5.0)?;
            session.set_text("wnd[0]/usr/ctxtNAST-LDEST", "MAIL")?;
            session.find_by_id("wnd[0]/tbar[0]/btn[3]")?.call("press", vec![])?; // BACK
            wait_sap(session, 5.0)?;

            output_table = session.find_by_id(output_table_id)?;
            let yellow_row = find_yellow_zpz3_row(&output_table)?;
            if yellow_row >= 0 {
                select_row(&output_table, yellow_row)?;
                session.find_by_id("wnd[0]/tbar[1]/btn[5]")?.call("press", vec![])?; // Further data
                wait_sap(session, 5.0)?;
                session.set_key("wnd[0]/usr/cmbNAST-VSZTP", "4")?;
                session.find_by_id("wnd[0]/tbar[0]/btn[3]")?.call("press", vec![])?; // BACK
                wait_sap(session, 5.0)?;
                if is_save {
                    session.find_by_id("wnd[0]/tbar[0]/btn[11]")?.call("press", vec![])?; // SAVE
                    output_info.push(format!("{material} {today} 新增ZPZ3完成"));
                }
                else {
                    output_info.push(format!("{material} {today} 新增ZPZ3完成(未保存)"));
                }
            } else {
                output_info.push(format!("{material} 新增ZPZ3失败"));
                logger::warn(app, &format!("{material} 设置 Communication 后未找到黄色 ZPZ3"));
                break;
            }
        } else {
            let date_hint = existing_time
                .as_deref()
                .map(|d| format!("，时间：{d}"))
                .unwrap_or_default();
            logger::info(app, &format!("{material} 无需新增 ZPZ3 {date_hint}"));
        }

        session.find_by_id("wnd[0]/tbar[0]/btn[3]")?.call("press", vec![])?; // BACK
    }

    if output_info.is_empty() {
        output_info.push("没有找到可以执行任何 ZPZ3 的操作".to_string());
    }
    logger::info(app, &format!("结果: {:?}", output_info));
    Ok(output_info)
}

/// 查找黄色状态的 ZPZ3 行
fn find_yellow_zpz3_row(table: &Disp) -> Result<i32, String> {
    let visible_count = table
        .get_i32("VisibleRowCount")
        .map(|c| c.abs())
        .unwrap_or(0);
    for j in 0..visible_count {
        let status = get_cell_icon(table, j, 0)?;
        let out_type = get_cell_text(table, j, 1)?;
        if out_type == "ZPZ3" && status == "S_TL_Y" {
            return Ok(j);
        }
    }
    Ok(-1)
}
