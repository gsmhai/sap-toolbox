//! 三个 SAP 业务操作，等价于原 test.py 中的函数：
//!   GetCustomerCreditInfo(session, CustomerCode)   -> FD33
//!   GetOrderAmount(session, OrderNo, onlyHead)     -> VA03
//!   ChangeOutBoundDelivery(session, OrderNo)       -> VL02N
//!
//! 每个操作接收 AppHandle，把进度信息输出到前端日志面板 + 日志文件。

use tauri::AppHandle;

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

/// ① 客户信用信息查询 (FD33)
pub fn get_customer_credit_info(
    app: &AppHandle,
    session: &Session,
    customer_code: &str,
) -> Result<CreditInfo, String> {
    logger::info(app, &format!("FD33 查询客户信用: {customer_code} ..."));
    session.maximize()?;
    session.start_transaction("FD33")?;
    wait_sap(session, 5.0)?;
    session.set_text("wnd[0]/usr/ctxtP_KUNBP", customer_code)?;
    session.send_vkey(0)?;
    wait_sap(session, 5.0)?;

    let total_credit_limit = parse_amount(&session.get_text(
        "wnd[0]/usr/tabsTABSTRIP_SEGMENTS/tabpTAB_1/ssub%_SUBSCREEN_SEGMENTS:ZF_FSCM_FD33_INFO_SHOW:1001/txtP_CLALL",
    )?);
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
) -> Result<f64, String> {
    logger::info(app, &format!("VA03 查询订单金额: {order_no} (onlyHead={only_head}) ..."));
    session.maximize()?;
    session.start_transaction("VA03")?;
    wait_sap(session, 5.0)?;
    session.set_text("wnd[0]/usr/ctxtVBAK-VBELN", order_no)?;
    session.send_vkey(0)?;
    wait_sap(session, 5.0)?;
    session.send_vkey(0)?;
    wait_sap(session, 5.0)?;

    let amount: f64;
    if only_head {
        amount = parse_amount(&session.get_text(
            "wnd[0]/usr/subSUBSCREEN_HEADER:SAPMV45A:4021/txtVBAK-NETWR",
        )?);
    } else {
        session
            .find_by_id("wnd[0]/usr/subSUBSCREEN_HEADER:SAPMV45A:4021/btnBT_HEAD")?
            .call("press", vec![])?;
        wait_sap(session, 5.0)?;
        session
            .find_by_id(r"wnd[0]/usr/tabsTAXI_TABSTRIP_HEAD/tabpT\05")?
            .call("select", vec![])?;

        let price_table = session.find_by_id(
            r"wnd[0]/usr/tabsTAXI_TABSTRIP_HEAD/tabpT\05/ssubSUBSCREEN_BODY:SAPLV69A:6201/tblSAPLV69ATCTRL_KONDITIONEN",
        )?;

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

        let rows = price_table.get_disp("Rows")?;
        let row_count = collection_count(&rows);
        let mut row: i32 = 0;
        for i in 0..row_count {
            let desc = get_cell_text(&price_table, i, desc_col)?;
            if desc.contains("Total amount") {
                row = i;
                break;
            }
        }
        amount = parse_amount(&get_cell_text(&price_table, row, col)?);
        session.send_vkey(3)?; // BACK
    }

    logger::info(app, &format!("订单 {order_no} 金额: {amount}"));
    Ok(amount)
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
    session.maximize()?;
    session.start_transaction("VL02N")?;
    wait_sap(session, 5.0)?;
    session.set_text("wnd[0]/usr/ctxtLIKP-VBELN", order_no)?;
    session.send_vkey(0)?;
    wait_sap(session, 5.0)?;

    let delivery_table_id = r"wnd[0]/usr/tabsTAXI_TABSTRIP_OVERVIEW/tabpT\01/ssubSUBSCREEN_BODY:SAPMV50A:1102/tblSAPMV50ATC_LIPS_OVER";
    let output_table_id = r"wnd[0]/usr/tblSAPDV70ATC_NAST3";

    let mut output_info: Vec<String> = Vec::new();
    let delivery_table = session.find_by_id(delivery_table_id)?;

    let rows = delivery_table.get_disp("Rows")?;
    let row_count = collection_count(&rows);

    for i in 0..row_count {
        let material = get_cell_text(&delivery_table, i, 1)?;
        if material.is_empty() {
            break;
        }
        select_row(&delivery_table, i)?;

        // 菜单：输出
        session
            .find_by_id("wnd[0]/mbar/menu[3]/menu[2]/menu[1]")?
            .call("select", vec![])?;
        wait_sap(session, 5.0)?;

        let mut output_table = session.find_by_id(output_table_id)?;

        let mut zpz3_list: Vec<(String, String)> = Vec::new();
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

            if out_type == "ZPZ3" {
                zpz3_list.push((status.clone(), proce_date.clone()));
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
                session.set_text("wnd[0]/usr/cmbNAST-VSZTP", "4")?;
                session.find_by_id("wnd[0]/tbar[0]/btn[3]")?.call("press", vec![])?; // BACK
                wait_sap(session, 5.0)?;
                if is_save {
                    session.find_by_id("wnd[0]/tbar[0]/btn[11]")?.call("press", vec![])?; // SAVE
                }
                output_info.push(format!("{material} Date:{proce_date} 修改ZPZ3完成"));
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
        for (status, pro_data) in &zpz3_list {
            if status == "S_TL_G" && pro_data == &today {
                need_create = false;
                logger::info(app, "今天已设置 ZPZ3");
                break;
            }
        }

        if need_create {
            if let Err(e) = set_cell_text(&output_table, end_row, 1, "ZPZ3") {
                logger::warn(app, &format!("设置 ZPZ3 失败: {e}"));
                return Ok(output_info);
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
                session.set_text("wnd[0]/usr/cmbNAST-VSZTP", "4")?;
                session.find_by_id("wnd[0]/tbar[0]/btn[3]")?.call("press", vec![])?; // BACK
                wait_sap(session, 5.0)?;
                if is_save {
                    session.find_by_id("wnd[0]/tbar[0]/btn[11]")?.call("press", vec![])?; // SAVE
                }
                output_info.push(format!("{material} Date:{today} 新增ZPZ3完成"));
            } else {
                output_info.push(format!("{material} 新增ZPZ3失败"));
                logger::warn(app, "设置 Communication 后未找到黄色 ZPZ3");
                return Ok(output_info);
            }
        } else {
            logger::info(app, "无需新增 ZPZ3");
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
