//! SAP GUI Scripting 的 COM 调用层。
//!
//! 通过 IDispatch 动态调用（late binding）访问 SAP GUI 对象模型：
//!   SAPGUI -> Application -> Children(i) Connection -> Children(j) Session
//!   Session.findById(id) -> GuiComponent (.Text / .text / .Value / .selected / sendVKey ...)
//!
//! 等价于原 test.py 中的 win32com.client.GetObject("SAPGUI") 用法。
//! 基于 windows 0.52 的 raw VARIANT（union）写法，参考社区成熟的 IDispatch late binding 实现。

use windows::core::{BSTR, GUID, PCWSTR};
use windows::Win32::Foundation::VARIANT_BOOL;
use windows::Win32::System::Com::{
    CoGetObject, CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, DISPATCH_FLAGS,
    DISPATCH_METHOD, DISPATCH_PROPERTYGET, DISPATCH_PROPERTYPUT, IDispatch, DISPPARAMS,
};
use windows::Win32::System::Variant::{
    VariantInit, VARIANT, VT_BSTR, VT_BOOL, VT_DISPATCH, VT_I4, VT_R8,
};

/// DISPID_PROPERTYPUT：用于属性写入（property-put）时 rgdispidNamedArgs 的取值
/// windows 0.52 的 crate 未导出该常量，这里按 COM 规范固定为 -3。
const DISPID_PROPERTYPUT: i32 = -3;

/// VARENUM 是元组结构体 `VARENUM(pub u16)`，不能在 match 臂中直接解构，
/// 这里导出常用变体的底层 u16 值用于匹配。
const VARENUM_BSTR: u16 = VT_BSTR.0;
const VARENUM_I4: u16 = VT_I4.0;
const VARENUM_R8: u16 = VT_R8.0;
const VARENUM_DISPATCH: u16 = VT_DISPATCH.0;

/// 包装一个 IDispatch 对象
#[derive(Clone)]
pub struct Disp {
    pub ptr: IDispatch,
}

unsafe impl Send for Disp {}
unsafe impl Sync for Disp {}

impl Disp {
    pub fn new(ptr: IDispatch) -> Self {
        Disp { ptr }
    }

    /// 核心动态调用
    fn invoke(&self, flags: DISPATCH_FLAGS, name: &str, args: Vec<VARIANT>) -> Result<VARIANT, String> {
        unsafe {
            let name_h = windows::core::HSTRING::from(name);
            let name_pcwstr = PCWSTR::from_raw(name_h.as_ptr());

            let mut dispid: i32 = 0;
            self.ptr
                .GetIDsOfNames(
                    &GUID::default(),
                    &name_pcwstr,
                    1,
                    0,
                    &mut dispid,
                )
                .map_err(|e| format!("GetIDsOfNames({name}) 失败: {e}"))?;

            // 参数逆序（COM 约定 rgvarg[0] 是最后一个参数）
            // 注意：DISPPARAMS.rgvarg 类型为 *mut VARIANT（VARIANTARG 即 VARIANT）
            let mut vargs: Vec<VARIANT> = args.into_iter().rev().collect();

            let mut named_arg = DISPID_PROPERTYPUT;
            let mut dp = DISPPARAMS {
                rgvarg: std::ptr::null_mut(),
                rgdispidNamedArgs: std::ptr::null_mut(),
                cArgs: 0,
                cNamedArgs: 0,
            };
            if !vargs.is_empty() {
                dp.cArgs = vargs.len() as u32;
                dp.rgvarg = vargs.as_mut_ptr();
                if flags == DISPATCH_PROPERTYPUT {
                    dp.cNamedArgs = 1;
                    dp.rgdispidNamedArgs = &mut named_arg;
                }
            }

            let mut result = VariantInit();
            self.ptr
                .Invoke(
                    dispid,
                    &GUID::default(),
                    0,
                    flags,
                    &dp,
                    Some(&mut result),
                    None,
                    None,
                )
                .map_err(|e| format!("调用 {name} 失败: {e}"))?;
            Ok(result)
        }
    }

    /// 读取属性
    pub fn get(&self, name: &str) -> Result<VARIANT, String> {
        self.invoke(DISPATCH_PROPERTYGET, name, vec![])
    }

    /// 设置属性
    pub fn set(&self, name: &str, value: VARIANT) -> Result<(), String> {
        self.invoke(DISPATCH_PROPERTYPUT, name, vec![value])?;
        Ok(())
    }

    /// 调用方法
    pub fn call(&self, name: &str, args: Vec<VARIANT>) -> Result<VARIANT, String> {
        self.invoke(DISPATCH_METHOD, name, args)
    }

    pub fn get_str(&self, name: &str) -> Result<String, String> {
        let v = self.get(name)?;
        variant_to_string(&v)
    }

    pub fn get_i32(&self, name: &str) -> Result<i32, String> {
        let v = self.get(name)?;
        variant_to_i32(&v)
    }

    pub fn get_disp(&self, name: &str) -> Result<Disp, String> {
        let v = self.get(name)?;
        variant_to_disp(&v)
    }

    /// 以「方法」方式调用（DISPATCH_METHOD）并返回 IDispatch 结果。
    /// 用于 GetScriptingEngine 这类在 typelib 中定义为 method 的成员。
    pub fn call_disp(&self, name: &str, args: Vec<VARIANT>) -> Result<Disp, String> {
        let v = self.call(name, args)?;
        variant_to_disp(&v)
    }

    /// 集合索引访问：Children(i) / Columns(i) / Rows(i)
    pub fn item(&self, index: i32) -> Result<Disp, String> {
        let v = self
            .call("Item", vec![i32_variant(index)])
            .or_else(|_| self.call("item", vec![i32_variant(index)]))?;
        variant_to_disp(&v)
    }
}

// ---------- VARIANT 辅助（0.52 raw union） ----------

/// 取得 VARIANT 内部结构体 VARIANT_0_0 与最内层联合体 VARIANT_0_0_0 的引用。
/// 注意：VARIANT_0.Anonymous 是 `ManuallyDrop<VARIANT_0_0>`，必须显式解引用。
fn variant_to_string(v: &VARIANT) -> Result<String, String> {
    unsafe {
        let s = &*v.Anonymous.Anonymous; // &VARIANT_0_0
        let u = &s.Anonymous; // &VARIANT_0_0_0 联合体
        match s.vt.0 {
            VARENUM_BSTR => {
                let b: &BSTR = &*u.bstrVal; // bstrVal: ManuallyDrop<BSTR>
                Ok(b.to_string())
            }
            VARENUM_I4 => Ok(u.lVal.to_string()),
            VARENUM_R8 => Ok(u.dblVal.to_string()),
            _ => {
                let b: &BSTR = &*u.bstrVal;
                Ok(b.to_string())
            }
        }
    }
}

fn variant_to_i32(v: &VARIANT) -> Result<i32, String> {
    unsafe {
        let s = &*v.Anonymous.Anonymous;
        let u = &s.Anonymous;
        match s.vt.0 {
            VARENUM_I4 => Ok(u.lVal),
            VARENUM_BSTR => {
                let b: &BSTR = &*u.bstrVal;
                b.to_string()
                    .parse::<i32>()
                    .map_err(|e| format!("字符串转整数失败: {e}"))
            }
            _ => Ok(0),
        }
    }
}

fn variant_to_disp(v: &VARIANT) -> Result<Disp, String> {
    unsafe {
        let s = &*v.Anonymous.Anonymous;
        let u = &s.Anonymous;
        if s.vt.0 == VARENUM_DISPATCH {
            let opt: &Option<IDispatch> = &*u.pdispVal; // pdispVal: ManuallyDrop<Option<IDispatch>>
            match opt {
                Some(disp) => return Ok(Disp::new(disp.clone())),
                None => return Err("VARIANT 中的 IDispatch 为空".to_string()),
            }
        }
        Err(format!("VARIANT 类型不是 IDispatch（vt={}）", s.vt.0))
    }
}

/// 公开版本：把 VARIANT 转为 IDispatch（供 sap_ops 使用）
pub fn variant_to_disp_public(v: &VARIANT) -> Result<Disp, String> {
    variant_to_disp(v)
}

// ---------- SAP 对象模型 ----------

/// 连接到 SAP GUI 并返回 Application 的 IDispatch
///
/// 等价于原 test.py 的 `win32com.client.GetObject("SAPGUI")`：
/// 通过 COM 运行对象表（ROT）取回正在运行的 SAP GUI 脚本对象（moniker），
/// 再调用 `GetScriptingEngine` 拿到 GuiApplication。
///
/// 注意：不能用 `CLSIDFromProgID("SAPGUI") + CoCreateInstance`。
/// "SAPGUI" 是 ROT 中的 moniker 名称，并非已注册的 ProgID，
/// 直接解析会报 `0x800401F3`（CO_E_CLASSSTRING / 无效的类字符串）。
pub fn connect_sap_gui() -> Result<Disp, String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let name_w = windows::core::HSTRING::from("SAPGUI");
    let name_pcwstr = PCWSTR::from_raw(name_w.as_ptr());
    let sap_gui: IDispatch = unsafe {
        CoGetObject(name_pcwstr, None).map_err(|e| format!(
            "无法连接 SAP GUI（CoGetObject(\"SAPGUI\") 失败: {e}）。\n\
             说明：报错“无效的语法(0x800401E4)”表示“SAPGUI”对象未注册到系统，通常是 SAP Logon 未运行或脚本功能未启用。\n\
             请依次确认：\n\
             1. SAP Logon 已启动，并已登录至少一个 SAP 会话；\n\
             2. SAP GUI 选项 -> 辅助功能与脚本 -> 脚本 -> 已勾选“启用脚本”；\n\
             3. 完成上述设置后重启 SAP Logon，再点击“刷新会话”。"
        ))?
    };
    let sap_gui_disp = Disp::new(sap_gui);
    // 注意：GetScriptingEngine 在 SAP typelib 中是 method（DISPATCH_METHOD），
    // pywin32 通过 typelib 判断后也是用 DISPATCH_METHOD 调用。这里先用方法调用，
    // 失败再回退属性读取，兼容 SAP 对 flag 的两种定义。
    let app = sap_gui_disp
        .call_disp("GetScriptingEngine", vec![])
        .or_else(|_| sap_gui_disp.get_disp("GetScriptingEngine"))?;
    Ok(app)
}

/// SAP 集合对象的 Count 属性
fn get_collection_count(d: &Disp) -> Result<i32, String> {
    if let Ok(v) = d.get("Count") {
        return variant_to_i32(&v);
    }
    if let Ok(children) = d.get_disp("Children") {
        if let Ok(v) = children.get("Count") {
            return variant_to_i32(&v);
        }
    }
    Err("无法读取集合数量".to_string())
}

/// 列举所有会话
pub fn list_sessions(app: &Disp) -> Result<Vec<(i32, i32, String)>, String> {
    let conn_count = get_collection_count(app)?;
    let mut out = Vec::new();
    for i in 0..conn_count {
        let conn = app.item(i)?;
        let desc_prefix = conn.get_str("ConnectionString").unwrap_or_default();
        let sess_count = get_collection_count(&conn)?;
        for j in 0..sess_count {
            let sess = conn.item(j)?;
            let desc = match sess.get_disp("Info") {
                Ok(info) => {
                    let sys = info.get_str("SystemName").unwrap_or_default();
                    let client = info.get_str("Client").unwrap_or_default();
                    let user = info.get_str("User").unwrap_or_default();
                    let txn = info.get_str("Transaction").unwrap_or_default();
                    format!("{sys}/{client}/{user}  事务:{txn}")
                }
                Err(_) => "session".to_string(),
            };
            let mut full = desc;
            if !desc_prefix.is_empty() {
                full = format!("{full}   [{desc_prefix}]");
            }
            out.push((i, j, full));
        }
    }
    Ok(out)
}

/// 获取指定会话
pub fn get_session(app: &Disp, conn_idx: i32, sess_idx: i32) -> Result<Disp, String> {
    let conn = app.item(conn_idx).map_err(|e| format!("连接 {conn_idx} 不存在: {e}"))?;
    conn.item(sess_idx)
        .map_err(|e| format!("会话 {sess_idx} 不存在: {e}"))
}

/// 会话包装
pub struct Session {
    disp: Disp,
}

impl Session {
    pub fn new(disp: Disp) -> Self {
        Session { disp }
    }

    pub fn find_by_id(&self, id: &str) -> Result<Disp, String> {
        let v = self.disp.call("findById", vec![str_variant(id)])?;
        variant_to_disp(&v)
    }

    pub fn start_transaction(&self, code: &str) -> Result<(), String> {
        self.disp.call("StartTransaction", vec![str_variant(code)])?;
        Ok(())
    }

    pub fn send_vkey(&self, key: i32) -> Result<(), String> {
        self.disp.call("sendVKey", vec![i32_variant(key)])?;
        Ok(())
    }

    pub fn maximize(&self) -> Result<(), String> {
        let wnd = self.find_by_id("wnd[0]")?;
        let _ = wnd.call("maximize", vec![]);
        Ok(())
    }

    pub fn is_busy(&self) -> bool {
        self.disp.get_i32("Busy").map(|v| v != 0).unwrap_or(false)
    }

    /// 读取元素文本（.Text -> .text -> .Value）
    pub fn get_text(&self, id: &str) -> Result<String, String> {
        let el = self.find_by_id(id)?;
        if let Ok(s) = el.get_str("Text") {
            if !s.is_empty() {
                return Ok(s);
            }
        }
        if let Ok(s) = el.get_str("text") {
            if !s.is_empty() {
                return Ok(s);
            }
        }
        el.get_str("Value")
    }

    /// 设置元素文本
    pub fn set_text(&self, id: &str, value: &str) -> Result<(), String> {
        let el = self.find_by_id(id)?;
        let v = str_variant(value);
        el.set("text", v.clone()).or_else(|_| el.set("Text", v))
    }
}

/// 构造一个 BSTR VARIANT
pub fn str_variant(s: &str) -> VARIANT {
    let wide: Vec<u16> = s.encode_utf16().collect();
    let b = BSTR::from_wide(&wide).expect("无法构造 BSTR（内存不足？）");
    unsafe {
        let mut v = VariantInit();
        // VARIANT_0.Anonymous 为 ManuallyDrop<VARIANT_0_0>，需显式解引用才能写字段
        let body = &mut *v.Anonymous.Anonymous;
        body.vt = VT_BSTR;
        body.Anonymous.bstrVal = std::mem::ManuallyDrop::new(b);
        v
    }
}

/// 构造一个 i32 VARIANT
pub fn i32_variant(n: i32) -> VARIANT {
    unsafe {
        let mut v = VariantInit();
        let body = &mut *v.Anonymous.Anonymous;
        body.vt = VT_I4;
        body.Anonymous.lVal = n;
        v
    }
}

/// 构造一个 bool VARIANT（VT_BOOL）
pub fn bool_variant(b: bool) -> VARIANT {
    unsafe {
        let mut v = VariantInit();
        let body = &mut *v.Anonymous.Anonymous;
        body.vt = VT_BOOL;
        // VARIANT_BOOL: -1 = VARIANT_TRUE, 0 = VARIANT_FALSE
        body.Anonymous.boolVal = VARIANT_BOOL(if b { -1 } else { 0 });
        v
    }
}

/// 等待 SAP 会话空闲
pub fn wait_sap(session: &Session, timeout_secs: f64) -> Result<(), String> {
    let start = std::time::Instant::now();
    loop {
        if !session.is_busy() {
            return Ok(());
        }
        if start.elapsed().as_secs_f64() > timeout_secs {
            return Err("等待 SAP 会话超时".to_string());
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

/// 释放 COM
pub fn cleanup_com() {
    unsafe { CoUninitialize() };
}

// ---------- 金额解析 ----------

/// 解析 SAP 输出的金额字符串
pub fn parse_amount(text: &str) -> f64 {
    if text.is_empty() {
        return 0.0;
    }
    let mut t = text.trim().replace(' ', "").replace(' ', "");
    if t.is_empty() {
        return 0.0;
    }
    let mut neg = false;
    if let Some(stripped) = t.strip_suffix('-') {
        neg = true;
        t = stripped.to_string();
    } else if let Some(stripped) = t.strip_prefix('-') {
        neg = true;
        t = stripped.to_string();
    }
    t = t.trim_start_matches('(').trim_end_matches(')').to_string();
    t = t.chars().filter(|c| c.is_ascii_digit() || *c == ',' || *c == '.').collect::<String>();
    if t.is_empty() {
        return 0.0;
    }
    if t.contains(',') && t.contains('.') {
        if t.rfind(',') > t.rfind('.') {
            t = t.replace('.', "").replace(',', ".");
        } else {
            t = t.replace(',', "");
        }
    } else if t.contains(',') {
        let parts: Vec<&str> = t.split(',').collect();
        if parts.len() == 2 && (1..=2).contains(&parts[1].len()) {
            t = t.replace(',', ".");
        } else {
            t = t.replace(',', "");
        }
    }
    let v: f64 = t.parse().unwrap_or(0.0);
    if neg {
        -v
    } else {
        v
    }
}
