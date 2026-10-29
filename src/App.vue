<script setup lang="ts">
import { ref, onMounted, nextTick } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { save } from "@tauri-apps/plugin-dialog";

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
const orderRows = ref<Array<{ orderId: string; amount: string }>>([]);
const orderSummary = ref("");

const deliveryId = ref("");
const noSave = ref(true);
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
const showLog = ref(true);
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

function toggleLog() {
  showLog.value = !showLog.value;
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
  orderRows.value = ids.map((orderId) => ({ orderId, amount: "查询中..." }));
  let total = 0;
  let count = 0;

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
  orderSummary.value = `合计（共 ${count} 个 Order ID）：${fmt(total)}`;
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
      isSave: !noSave.value,
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
    <!-- 顶部栏 -->
    <div class="topbar">
      <div class="session-row">
        <span class="label">SAP 会话:</span>
        <select v-model.number="selectedIndex" class="session-select">
          <option v-for="(s, i) in sessions" :key="i" :value="i">
            [{{ s.connIdx }}-{{ s.sessIdx }}] {{ s.desc }}
          </option>
        </select>
        <button class="btn" @click="refreshSessions">刷新会话</button>
        <span class="conn-status" :class="connStatus === '已连接' ? 'ok' : 'bad'">{{ connStatus }}</span>
      </div>
      <button class="btn" @click="toggleLog">{{ showLog ? "隐藏日志" : "显示日志" }}</button>
    </div>

    <!-- 主体：左侧固定宽度功能区 + 右侧日志 -->
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
            <button class="btn primary" :disabled="busy" @click="runCredit">查询信用</button>
            <div class="table-col">
              <table class="data-table credit-table">
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
              <button class="btn small" :disabled="!creditRows[0].value" @click="copyText(creditTableText(), '信用表格')">
                复制表格
              </button>
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
                rows="6"
                @input="filterDigitsMulti"
              ></textarea>
            </div>
            <button class="btn primary" :disabled="busy" @click="runOrders">批量查询金额</button>
            <div class="table-col">
              <table class="data-table order-table">
                <thead>
                  <tr><th>Order ID</th><th>金额 / 返回值</th></tr>
                </thead>
                <tbody>
                  <tr v-for="(row, i) in orderRows" :key="i">
                    <td>{{ row.orderId }}</td>
                    <td>{{ row.amount }}</td>
                  </tr>
                  <tr v-if="orderRows.length > 0" class="summary">
                    <td class="bold red">{{ orderSummary }}</td>
                    <td class="bold red"></td>
                  </tr>
                  <tr v-if="orderRows.length === 0">
                    <td colspan="2" class="empty">（未查询）</td>
                  </tr>
                </tbody>
              </table>
              <div class="btn-row">
                <button class="btn small" :disabled="orderRows.length === 0" @click="copyText(orderTableText(), '订单表格')">复制表格</button>
                <button class="btn small" :disabled="orderRows.length === 0" @click="exportOrders">导出 CSV</button>
              </div>
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
                  <input type="checkbox" v-model="noSave" />
                  不保存
                </label>
              </div>
              <input
                v-model="deliveryId"
                placeholder="例如 40844941"
                @input="filterDigitsDelivery"
                @keyup.enter="runDelivery"
              />
            </div>
            <button class="btn primary" :disabled="busy" @click="runDelivery">修改交货单</button>
            <div class="result-box">
              <div class="result-title">执行结果</div>
              <pre class="result-text">{{ deliveryResult }}</pre>
            </div>
          </div>
        </div>
      </div>

      <!-- 右侧日志面板 -->
      <div v-if="showLog" class="log-panel">
        <div class="log-header">
          <span>运行日志</span>
          <span class="log-dir" v-if="logDir">{{ logDir }}</span>
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
  --log-bg: #1b1f27;
  --log-text: #d4d4d4;
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
  flex-direction: column;
  height: 100vh;
}

/* ===== 顶部栏 ===== */
.topbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 14px;
  background: #fff;
  border-bottom: 1px solid var(--border);
  flex-shrink: 0;
}

.session-row {
  display: flex;
  align-items: center;
  gap: 8px;
  flex: 1;
  min-width: 0;
}

.label {
  font-weight: 600;
  color: var(--muted);
  white-space: nowrap;
}

.session-select {
  min-width: 380px;
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
  min-width: 130px;
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

/* 主界面固定宽度：缩放窗口时不变，缩太小则整体被裁掉 */
.main-area {
  width: 800px;
  flex-shrink: 0;
  padding: 14px;
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
  align-items: flex-start;
}

.input-col {
  display: flex;
  flex-direction: column;
  gap: 6px;
  width: 210px;
  flex-shrink: 0;
}

.label-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
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
  padding: 7px 10px;
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

/* ===== 表格 ===== */
.table-col {
  width: 388px;
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  gap: 8px;
  align-items: flex-end;
}

.data-table {
  width: 100%;
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

.btn-row {
  display: flex;
  gap: 8px;
}

/* ===== 交货单结果 ===== */
.result-box {
  width: 388px;
  flex-shrink: 0;
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
.log-panel {
  flex: 1;
  min-width: 200px;
  border-left: 1px solid var(--border);
  background: var(--log-bg);
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.log-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding: 8px 12px;
  background: #232833;
  color: #e0e0e0;
  font-weight: 600;
  border-bottom: 1px solid #333a48;
  flex-shrink: 0;
}

.log-dir {
  font-size: 10px;
  color: #7c8698;
  font-weight: 400;
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
  color: #6a9955;
  margin-right: 6px;
}

.log-level {
  margin-right: 6px;
  font-weight: 600;
}

.log-msg {
  color: var(--log-text);
}

.log-line.info .log-level { color: #569cd6; }
.log-line.warn .log-level { color: #dcdcaa; }
.log-line.error .log-level { color: #f14c4c; }
.log-line.error .log-msg { color: #f14c4c; }

.empty {
  color: #888;
  text-align: center;
  padding: 20px;
}
</style>
