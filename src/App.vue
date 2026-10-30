<script setup lang="ts">
import { ref, onMounted, nextTick } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { save } from "@tauri-apps/plugin-dialog";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";

// ---------- 布局常量 ----------
const MAIN_W = 760; // 主界面宽度
const LOG_DEFAULT_W = 490; // 显示日志时窗口加宽的量
const SESSION_H = 70; // SAP 会话栏展开/收起时窗口高度变化量

// ---------- 状态 ----------
const sessions = ref<Array<{ connIdx: number; sessIdx: number; desc: string }>>([]);
const selectedIndex = ref(0);
const connStatus = ref<"未连接" | "已连接">("未连接");

function currentSession(): { connIdx: number; sessIdx: number } {
  const s = sessions.value[selectedIndex.value];
  if (s) {
    return { connIdx: s.connIdx, sessIdx: s.sessIdx };
  }
  return { connIdx: 0, sessIdx: 0 };
}

const customerCode = ref("");
const orderIds = ref("");
const onlyHead = ref(true);
const batchMode = ref(false); // 调试用：切换"单次进入 VA03 批量查询"与"逐条进入 VA03 查询"
const orderRows = ref<Array<{ orderId: string; amount: string }>>([]);
const orderSummary = ref("合计（共 0 个）：0.00");

const deliveryId = ref("");
const autoSave = ref(true);
const deliveryResult = ref("");

const busy = ref(false);

// ---------- 数字输入过滤（粘贴带字母时智能剔除非数字字符） ----------
function filterDigits(e: Event) {
  const el = e.target as HTMLInputElement;
  const clean = el.value.replace(/\D/g, "");
  if (clean !== el.value) el.value = clean;
  customerCode.value = clean;
}

function filterDigitsMulti(e: Event) {
  const el = e.target as HTMLTextAreaElement;
  const clean = el.value.replace(/[^\d\n]/g, "");
  if (clean !== el.value) el.value = clean;
  orderIds.value = clean;
}

function filterDigitsDelivery(e: Event) {
  const el = e.target as HTMLInputElement;
  const clean = el.value.replace(/\D/g, "");
  if (clean !== el.value) el.value = clean;
  deliveryId.value = clean;
}

// ---------- 日志 ----------
const logEntries = ref<Array<{ time: string; level: string; message: string }>>([]);
const showLog = ref(false); // 日志默认隐藏
const logDir = ref("");
const logBox = ref<HTMLElement | null>(null);

function appendLog(entry: { time: string; level: string; message: string }) {
  logEntries.value.push(entry);
  if (logEntries.value.length > 2000) {
    logEntries.value = logEntries.value.slice(-2000);
  }
  nextTick(() => {
    if (logBox.value) {
      logBox.value.scrollTop = logBox.value.scrollHeight;
    }
  });
}

// 隐藏日志：窗口宽度同步缩掉日志区域；显示：加宽回来
async function toggleLog() {
  const win = getCurrentWindow();
  const next = !showLog.value;
  let targetW = 0;
  let h = 0;
  try {
    const size = await win.innerSize();
    const scale = (await win.scaleFactor()) || 1;
    const w = Math.round(size.width / scale);
    h = Math.round(size.height / scale);
    if (next) {
      targetW = w + LOG_DEFAULT_W;
    } else {
      const logW = Math.max(w - MAIN_W, 0);
      targetW = Math.max(w - logW, 560);
    }
  } catch (e) {
    appendLog({ time: now(), level: "ERROR", message: `读取窗口尺寸失败: ${e}` });
  }
  showLog.value = next;
  if (targetW > 0) {
    try {
      await win.setSize(new LogicalSize(targetW, h));
    } catch (e) {
      appendLog({ time: now(), level: "ERROR", message: `调整窗口大小失败: ${e}` });
    }
  }
}

// ---------- SAP 会话栏：默认收起，点击浮动条展开/收起，窗口高度随之变化 ----------
const sessionOpen = ref(false);

async function toggleSession() {
  const win = getCurrentWindow();
  const next = !sessionOpen.value;
  try {
    const size = await win.innerSize();
    const scale = (await win.scaleFactor()) || 1;
    const w = Math.round(size.width / scale);
    const h = Math.round(size.height / scale);
    const targetH = Math.max(h + (next ? SESSION_H : -SESSION_H), 290); // 最小高度与窗口 minHeight 保持一致
    sessionOpen.value = next;
    await win.setSize(new LogicalSize(w, targetH));
  } catch (e) {
    sessionOpen.value = next;
    appendLog({ time: now(), level: "ERROR", message: `调整窗口大小失败: ${e}` });
  }
}

// ---------- 会话 ----------
async function refreshSessions() {
  try {
    const list = await invoke<Array<[number, number, string]>>("refresh_sessions");
    sessions.value = list.map(([connIdx, sessIdx, desc]) => ({ connIdx, sessIdx, desc }));
    if (list.length > 0) {
      connStatus.value = "已连接";
      selectedIndex.value = 0;
    } else {
      connStatus.value = "未连接";
    }
  } catch (e) {
    connStatus.value = "未连接";
    appendLog({ time: now(), level: "ERROR", message: `读取 SAP 会话失败: ${e}` });
  }
}

function now(): string {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

// ---------- 信用查询：固定五行表格，查询后直接更新数值 ----------
const creditRows = ref([
  { name: "信用总额 TotalCreditLimit", value: "", bold: false },
  { name: "未清订单 OpenOrders", value: "", bold: false },
  { name: "未清交货 OpenDelivery", value: "", bold: false },
  { name: "应收总额 TotalReceivables", value: "", bold: false },
  { name: "可用额度 (信用 - 未清交货 - 应收)", value: "", bold: true },
]);

async function runCredit() {
  if (!customerCode.value.trim()) {
    appendLog({ time: now(), level: "WARN", message: "请先输入 Customer Code" });
    return;
  }
  busy.value = true;
  try {
    const res = await invoke<any>("query_credit", {
      customerCode: customerCode.value.trim(),
      connIdx: currentSession().connIdx,
      sessIdx: currentSession().sessIdx,
    });
    creditRows.value[0].value = fmt(res.total_credit_limit);
    creditRows.value[1].value = fmt(res.open_orders);
    creditRows.value[2].value = fmt(res.open_delivery);
    creditRows.value[3].value = fmt(res.total_receivables);
    creditRows.value[4].value = fmt(res.available_delivery);
  } catch (e) {
    appendLog({ time: now(), level: "ERROR", message: `查询信用失败: ${e}` });
  } finally {
    busy.value = false;
  }
}

function fmt(n: number): string {
  return n.toLocaleString("zh-CN", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
}

// ---------- 订单金额批量查询 ----------
async function runOrders() {
  const ids = orderIds.value
    .split(/\r?\n/)
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
  if (ids.length === 0) {
    appendLog({ time: now(), level: "WARN", message: "请至少输入一个 Order ID" });
    return;
  }
  busy.value = true;
  const t0 = performance.now();
  const modeLabel = batchMode.value ? "批量·单次进入 VA03" : "单条·逐条进入 VA03";
  orderRows.value = ids.map((orderId) => ({ orderId, amount: "查询中..." }));
  let total = 0;
  let count = 0;

  try {
    if (batchMode.value) {
      // 新方案：只进入一次 VA03，在事务内逐个切换订单号
      const res = await invoke<Array<[string, number]>>("query_order_amounts", {
        orderNos: ids,
        onlyHead: onlyHead.value,
        connIdx: currentSession().connIdx,
        sessIdx: currentSession().sessIdx,
      });
      for (const [orderId, val] of res) {
        const row = orderRows.value.find((r) => r.orderId === orderId);
        if (row) row.amount = fmt(val);
        total += val;
        count++;
      }
    } else {
      // 原方案：每个订单号都重新进入一次 VA03
      for (const id of ids) {
        try {
          const val = await invoke<number>("query_order_amount", {
            orderNo: id,
            onlyHead: onlyHead.value,
            connIdx: currentSession().connIdx,
            sessIdx: currentSession().sessIdx,
          });
          const row = orderRows.value.find((r) => r.orderId === id && r.amount === "查询中...");
          if (row) row.amount = fmt(val);
          total += val;
          count++;
        } catch (e) {
          const row = orderRows.value.find((r) => r.orderId === id && r.amount === "查询中...");
          if (row) row.amount = `ERROR: ${e}`;
        }
      }
    }
  } catch (e) {
    appendLog({ time: now(), level: "ERROR", message: `批量查询失败: ${e}` });
  }

  const elapsed = (performance.now() - t0) / 1000;
  orderSummary.value = `合计（共 ${count} 个）：${fmt(total)}`;
  appendLog({
    time: now(),
    level: "INFO",
    message: `订单查询完成 [${modeLabel}]，${count} 个，耗时 ${elapsed.toFixed(2)} 秒`,
  });
  busy.value = false;
}

// ---------- 交货单修改 ----------
async function runDelivery() {
  if (!deliveryId.value.trim()) {
    appendLog({ time: now(), level: "WARN", message: "请先输入 Delivery ID" });
    return;
  }
  busy.value = true;
  deliveryResult.value = "执行中，请稍候...（SAP 界面会被自动操作，请勿手工点击）";
  try {
    const res = await invoke<string[]>("modify_delivery", {
      orderNo: deliveryId.value.trim(),
      isSave: autoSave.value,
      connIdx: currentSession().connIdx,
      sessIdx: currentSession().sessIdx,
    });
    deliveryResult.value = res.length > 0 ? res.join("\n") : "（无返回结果）";
  } catch (e) {
    deliveryResult.value = `错误: ${e}`;
  } finally {
    busy.value = false;
  }
}

// ---------- 复制表格 ----------
async function copyText(text: string, label: string) {
  await navigator.clipboard.writeText(text);
  appendLog({ time: now(), level: "INFO", message: `${label}已复制到剪贴板` });
}

function creditTableText(): string {
  const lines = ["项目\t数值"];
  for (const item of creditRows.value) {
    lines.push(`${item.name}\t${item.value || "—"}`);
  }
  return lines.join("\n");
}

function orderTableText(): string {
  const lines = ["Order ID\t金额 / 返回值"];
  for (const row of orderRows.value) {
    lines.push(`${row.orderId}\t${row.amount}`);
  }
  return lines.join("\n");
}

async function exportOrders() {
  if (orderRows.value.length === 0) return;
  const path = await save({
    defaultPath: "order_amount.csv",
    filters: [{ name: "CSV", extensions: ["csv"] }],
  });
  if (!path) return;
  let content = "OrderId,Amount\n";
  for (const row of orderRows.value) {
    content += `"${row.orderId}","${row.amount}"\n`;
  }
  await invoke("export_csv", { path, content });
}

// ---------- 生命周期 ----------
onMounted(async () => {
  await listen("log://entry", (event) => {
    const payload = event.payload as { time: string; level: string; message: string };
    appendLog(payload);
  });

  try {
    logDir.value = await invoke<string>("get_log_dir");
  } catch (e) {
    logDir.value = "";
  }

  await refreshSessions();
});
</script>

<template>
  <div class="app">
    <div class="left-col">
      <!-- 顶部细工具条：点击后整排（SAP 会话/刷新会话/日志开关）一起滑下来 -->
      <div class="toolbar">
      <div class="toolbar-row">
        <div class="toolbar-main" @click="toggleSession" title="显示 / 隐藏会话与日志工具">
          <div class="grab-bar"></div>
        </div>
      </div>
      <div class="session-panel" :class="{ open: sessionOpen }">
        <div class="session-card">
          <div class="session-row">
            <div class="session-tools">
              <span class="label">SAP 会话:</span>
              <select v-model.number="selectedIndex" class="session-select">
                <option v-for="(s, i) in sessions" :key="i" :value="i">
                  [{{ s.connIdx }}-{{ s.sessIdx }}] {{ s.desc }}
                </option>
              </select>
              <button class="btn" @click="refreshSessions">刷新会话</button>
              <span class="conn-status" :class="connStatus === '已连接' ? 'ok' : 'bad'">{{ connStatus }}</span>
            </div>
            <button class="btn log-btn" @click="toggleLog">{{ showLog ? "隐藏日志" : "显示日志" }}</button>
          </div>
        </div>
      </div>
    </div>

    <!-- 主体：左侧主界面 + 右侧日志 -->
    <div class="body">
      <div class="main-area">
        <!-- 客户信用信息查询 -->
        <div class="card">
          <div class="card-title">客户信用信息查询 (FD33)</div>
          <div class="card-body">
            <div class="input-col">
              <label>Customer Code</label>
              <input
                v-model="customerCode"
                placeholder="例如 0070028763"
                @input="filterDigits"
                @keyup.enter="runCredit"
              />
            </div>
            <div class="btn-col">
              <button class="btn primary" :disabled="busy" @click="runCredit">查询信用</button>
              <button class="btn small" :disabled="!creditRows[0].value" @click="copyText(creditTableText(), '信用表格')">
                复制表格
              </button>
            </div>
            <div class="table-col">
              <table class="data-table">
                <thead>
                  <tr><th>项目</th><th>数值</th></tr>
                </thead>
                <tbody>
                  <tr v-for="item in creditRows" :key="item.name" :class="{ summary: item.bold }">
                    <td :class="{ bold: item.bold }">{{ item.name }}</td>
                    <td :class="{ bold: item.bold, red: item.bold }">{{ item.value || "—" }}</td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
        </div>

        <!-- 订单金额批量查询 -->
        <div class="card">
          <div class="card-title">订单金额批量查询 (VA03)</div>
          <div class="card-body">
            <div class="input-col">
              <div class="label-row">
                <label>Order ID 列表</label>
                <label class="checkbox">
                  <input type="checkbox" v-model="onlyHead" />
                  仅取抬头净值
                </label>
              </div>
              <textarea
                v-model="orderIds"
                placeholder="40844941&#10;41129429&#10;9071842"
                rows="5"
                @input="filterDigitsMulti"
              ></textarea>
            </div>
            <div class="btn-col">
              <label class="checkbox mode-toggle" title="切换：批量=只进入一次 VA03 逐个切换订单号；单条=每个订单号都重新进入 VA03">
                <input type="checkbox" v-model="batchMode" />
                批量模式
              </label>
              <button class="btn primary" :disabled="busy" @click="runOrders">批量查询金额</button>
              <button class="btn small" :disabled="orderRows.length === 0" @click="copyText(orderTableText(), '订单表格')">复制表格</button>
              <button class="btn small" :disabled="orderRows.length === 0" @click="exportOrders">导出 CSV</button>
            </div>
            <div class="table-col">
              <table class="data-table">
                <thead>
                  <tr><th>Order ID</th><th>金额 / 返回值</th></tr>
                </thead>
                <tbody>
                  <tr v-for="(row, i) in orderRows" :key="i">
                    <td>{{ row.orderId }}</td>
                    <td>{{ row.amount }}</td>
                  </tr>
                  <tr v-if="orderRows.length === 0" class="placeholder-row">
                    <td colspan="2" class="empty">（未查询）</td>
                  </tr>
                  <tr class="summary">
                    <td class="bold red">{{ orderSummary }}</td>
                    <td class="bold red"></td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
        </div>

        <!-- 修改交货单 -->
        <div class="card">
          <div class="card-title">修改交货单 (VL02N)</div>
          <div class="card-body">
            <div class="input-col">
              <div class="label-row">
                <label>Delivery ID</label>
                <label class="checkbox">
                  <input type="checkbox" v-model="autoSave" />
                  自动保存
                </label>
              </div>
              <input
                v-model="deliveryId"
                placeholder="例如 40844941"
                @input="filterDigitsDelivery"
                @keyup.enter="runDelivery"
              />
            </div>
            <div class="btn-col">
              <button class="btn primary" :disabled="busy" @click="runDelivery">修改交货单</button>
            </div>
            <div class="result-box">
              <div class="result-title">执行结果</div>
              <pre class="result-text">{{ deliveryResult }}</pre>
            </div>
          </div>
        </div>
      </div>
    </div>
    </div>

    <!-- 右侧日志面板：从标题栏一直到底部，与主界面并排 -->
    <div v-if="showLog" class="log-panel">
      <div class="log-header">
        <span v-if="logDir">{{ logDir }}</span>
        <span v-else>（暂无日志目录）</span>
      </div>
      <div class="log-body" ref="logBox">
        <div v-for="(entry, i) in logEntries" :key="i" class="log-line" :class="entry.level.toLowerCase()">
          <span class="log-time">{{ entry.time }}</span>
          <span class="log-level">[{{ entry.level }}]</span>
          <span class="log-msg">{{ entry.message }}</span>
        </div>
        <div v-if="logEntries.length === 0" class="empty">（暂无日志）</div>
      </div>
    </div>
  </div>
</template>

<style>
:root {
  --bg: #f3f5f9;
  --card-bg: #ffffff;
  --border: #e2e6ee;
  --text: #1f2933;
  --muted: #64748b;
  --primary: #2563eb;
  --primary-dark: #1d4ed8;
  --red: #dc2626;
  --green: #16a34a;
  --log-bg: #eaeff7;
  --log-text: #334155;
}

* {
  box-sizing: border-box;
  margin: 0;
  padding: 0;
}

body {
  font-family: "Microsoft YaHei", "Segoe UI", sans-serif;
  font-size: 13px;
  color: var(--text);
  background: var(--bg);
  overflow: hidden;
}

.app {
  display: flex;
  flex-direction: row;
  height: 100vh;
}

/* 左侧：工具条 + 主界面，宽度固定 760，缩小时先让日志列收窄 */
.left-col {
  flex-grow: 0;
  flex-shrink: 1;
  flex-basis: 760px;
  min-width: 540px;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

/* ===== 顶部：细工具条 + 会话栏 ===== */
.toolbar {
  flex-shrink: 0;
  position: relative;
  z-index: 20;
}

.toolbar-row {
  display: flex;
}

/* 必须 flex 才能让 760 基准宽度生效（日志开关贴主界面右缘） */
.toolbar-main {
  flex: 1 1 auto;
  min-width: 0;
  position: relative;
  height: 12px;
  display: flex;
  justify-content: center;
  align-items: flex-start;
  cursor: pointer;
}

/* 细白条：紧贴标题栏，悬停时下滑一点并出现阴影 */
.grab-bar {
  margin-top: 4px;
  width: 92px;
  height: 4px;
  border-radius: 999px;
  background: #d3d9e3;
  transition: transform 0.22s ease, background 0.22s ease, box-shadow 0.22s ease;
}

.toolbar-main:hover .grab-bar {
  background: #b7c2d2;
  transform: translateY(3px);
  box-shadow: 0 3px 8px rgba(15, 23, 42, 0.18);
}

/* 会话栏：点击工具条后整排一起滑下来 */
.session-panel {
  width: 100%;
  max-width: 100%;
  min-width: 0;
  max-height: 0;
  overflow: hidden;
  transition: max-height 0.25s ease;
}

.session-panel.open {
  max-height: 90px;
}

.session-card {
  margin: 4px 14px 10px;
  padding: 10px 14px;
  background: #fff;
  border: 1px solid var(--border);
  border-radius: 12px;
  box-shadow: 0 1px 3px rgba(15, 23, 42, 0.05);
  transform: translateY(-24px);
  opacity: 0;
  transition: transform 0.25s ease, opacity 0.25s ease, box-shadow 0.25s ease;
}

.session-panel.open .session-card {
  transform: translateY(0);
  opacity: 1;
  box-shadow: 0 6px 16px rgba(15, 23, 42, 0.12);
}

.session-tools {
  display: flex;
  align-items: center;
  gap: 8px;
  flex: 1 1 auto;
  min-width: 0;
  overflow: hidden;
}

/* 抽屉里的日志开关，贴在抽屉右端 */
.log-btn {
  flex: 0 0 auto;
  margin-left: auto;
  padding: 6px 14px;
  font-size: 12px;
  border-radius: 8px;
}

.session-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  width: 100%;
  min-width: 0;
}

.label {
  font-weight: 600;
  color: var(--muted);
  white-space: nowrap;
}

.session-select {
  flex: 0 1 240px;
  min-width: 120px;
  padding: 6px 10px;
  border: 1px solid #d5dae3;
  border-radius: 8px;
  background: #fff;
  font-size: 13px;
  color: var(--text);
}

.conn-status {
  font-weight: 600;
  padding: 3px 12px;
  border-radius: 999px;
  font-size: 12px;
}
.conn-status.ok {
  color: var(--green);
  background: #e8f7ee;
}
.conn-status.bad {
  color: var(--red);
  background: #fdeaea;
}

/* ===== 按钮 ===== */
.btn {
  padding: 7px 16px;
  border: 1px solid #d5dae3;
  border-radius: 8px;
  background: #fff;
  cursor: pointer;
  color: var(--text);
  font-size: 13px;
  font-family: inherit;
  transition: all 0.15s ease;
  white-space: nowrap;
}
.btn:hover {
  background: #f1f5fb;
  border-color: #b9c6dd;
}
.btn:active {
  transform: translateY(1px);
}
.btn:disabled {
  color: #a8b0bc;
  background: #f6f7f9;
  border-color: #e5e8ee;
  cursor: not-allowed;
  transform: none;
}
.btn.primary {
  height: 42px;
  font-weight: 600;
  font-size: 14px;
  color: #fff;
  background: linear-gradient(135deg, #3b82f6 0%, var(--primary) 60%, var(--primary-dark) 100%);
  border: none;
  box-shadow: 0 2px 6px rgba(37, 99, 235, 0.35);
}
.btn.primary:hover:not(:disabled) {
  background: linear-gradient(135deg, #2f74ee 0%, var(--primary-dark) 100%);
  box-shadow: 0 3px 10px rgba(37, 99, 235, 0.45);
}
.btn.small {
  padding: 5px 14px;
  font-size: 12px;
  border-radius: 999px;
}

/* ===== 主体布局 ===== */
.body {
  display: flex;
  flex: 1;
  overflow: hidden;
}

/* 主界面：占满左侧列 */
.main-area {
  flex: 1 1 auto;
  min-width: 0;
  padding: 0 14px 14px;
  display: flex;
  flex-direction: column;
  gap: 14px;
  overflow: hidden;
}

/* ===== 卡片 ===== */
.card {
  background: var(--card-bg);
  border: 1px solid var(--border);
  border-radius: 12px;
  padding: 14px;
  box-shadow: 0 1px 3px rgba(15, 23, 42, 0.05);
}

.card-title {
  font-weight: 700;
  font-size: 14px;
  color: #334155;
  margin-bottom: 12px;
  padding-left: 10px;
  border-left: 3px solid var(--primary);
  line-height: 1.2;
}

.card-body {
  display: flex;
  gap: 14px;
  align-items: stretch;
}

.input-col {
  display: flex;
  flex-direction: column;
  gap: 6px;
  width: 180px;
  flex-shrink: 0;
}

.label-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 6px;
  min-height: 20px;
}

.input-col label {
  font-weight: 600;
  color: var(--muted);
  font-size: 12px;
  white-space: nowrap;
}

.input-col input,
.input-col textarea {
  width: 100%;
  padding: 7px 9px;
  border: 1px solid #d5dae3;
  border-radius: 8px;
  font-size: 13px;
  font-family: inherit;
  color: var(--text);
  transition: border-color 0.15s ease, box-shadow 0.15s ease;
  resize: none;
}

.input-col input:focus,
.input-col textarea:focus {
  outline: none;
  border-color: var(--primary);
  box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.12);
}

.checkbox {
  display: flex;
  align-items: center;
  gap: 5px;
  font-weight: 400 !important;
  color: var(--text) !important;
  cursor: pointer;
  font-size: 12px;
  white-space: nowrap;
}

.checkbox input {
  accent-color: var(--primary);
  cursor: pointer;
}

/* 按钮竖排列 */
.btn-col {
  display: flex;
  flex-direction: column;
  gap: 8px;
  width: 124px;
  flex-shrink: 0;
}

.mode-toggle {
  align-self: flex-start;
}

/* ===== 表格 ===== */
.table-col {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-width: 368px;
}

.data-table {
  width: 100%;
  height: 100%;
  border-collapse: separate;
  border-spacing: 0;
  font-size: 13px;
  border: 1px solid var(--border);
  border-radius: 8px;
  overflow: hidden;
}

.data-table th,
.data-table td {
  padding: 7px 12px;
  text-align: left;
  border-bottom: 1px solid #eef1f6;
}

.data-table th {
  background: linear-gradient(180deg, #f8fafd 0%, #eef2f8 100%);
  font-weight: 600;
  color: #475569;
  font-size: 12px;
}

.data-table tbody tr:last-child td {
  border-bottom: none;
}

.data-table tbody tr:nth-child(even):not(.summary) {
  background: #fafbfd;
}

.data-table tr.summary td {
  background: #fff7ed;
  border-top: 1px solid #f3e3cd;
}

.data-table td.bold {
  font-weight: 700;
}
.data-table td.red {
  color: var(--red);
}
.data-table td.empty {
  color: #9aa2ab;
  text-align: center;
}

/* 未查询时表格与左侧输入框（5 行）等高 */
.data-table tr.placeholder-row td {
  height: 68px;
  vertical-align: middle;
}

/* ===== 交货单结果 ===== */
.result-box {
  flex: 1;
  min-width: 368px;
  border: 1px solid var(--border);
  border-radius: 8px;
  overflow: hidden;
  align-self: stretch;
}

.result-title {
  font-weight: 600;
  padding: 6px 12px;
  background: linear-gradient(180deg, #f8fafd 0%, #eef2f8 100%);
  border-bottom: 1px solid var(--border);
  font-size: 12px;
  color: #475569;
}

.result-text {
  padding: 10px 12px;
  white-space: pre-wrap;
  word-break: break-all;
  font-family: inherit;
  font-size: 13px;
  min-height: 56px;
}

/* ===== 右侧日志面板 ===== */
/* 水平放大时优先撑宽日志；缩小到归零后主界面才开始收缩 */
.log-panel {
  flex-grow: 1;
  flex-shrink: 100;
  flex-basis: 240px;
  min-width: 0;
  border-left: 1px solid var(--border);
  background: var(--log-bg);
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.log-header {
  display: flex;
  align-items: center;
  padding: 6px 12px;
  background: linear-gradient(180deg, #e2e9f4 0%, #d9e3f1 100%);
  color: #64748b;
  font-size: 11px;
  font-family: "Consolas", "Courier New", monospace;
  border-bottom: 1px solid #dde4ee;
  flex-shrink: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.log-body {
  flex: 1;
  overflow-y: auto;
  padding: 8px;
  font-family: "Consolas", "Courier New", monospace;
  font-size: 12px;
  line-height: 1.6;
}

.log-line {
  white-space: pre-wrap;
  word-break: break-all;
}

.log-time {
  color: #94a3b8;
  margin-right: 6px;
}

.log-level {
  margin-right: 6px;
  font-weight: 600;
}

.log-msg {
  color: var(--log-text);
}

.log-line.info .log-level { color: #2563eb; }
.log-line.warn .log-level { color: #b45309; }
.log-line.error .log-level { color: var(--red); }
.log-line.error .log-msg { color: var(--red); }

.empty {
  color: #9aa2ab;
  text-align: center;
  padding: 20px;
}
</style>
