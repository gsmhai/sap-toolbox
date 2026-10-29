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
const creditData = ref<Array<{ name: string; value: string; bold?: boolean }>>([]);

const orderIds = ref("");
const onlyHead = ref(true);
const orderRows = ref<Array<{ orderId: string; amount: string }>>([]);
const orderSummary = ref("");

const deliveryId = ref("");
const deliveryResult = ref("");

const busy = ref(false);

// ---------- 日志 ----------
const logEntries = ref<Array<{ time: string; level: string; message: string }>>([]);
const showLog = ref(true);
const logDir = ref("");
const logBox = ref<HTMLElement | null>(null);

// ---------- 日志相关 ----------
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

// ---------- 信用查询 ----------
const creditItems = [
  "信用总额 TotalCreditLimit",
  "未清订单 OpenOrders",
  "未清交货 OpenDelivery",
  "应收总额 TotalReceivables",
  "可用额度(信用-未清交货-应收)",
];

async function runCredit() {
  if (!customerCode.value.trim()) {
    appendLog({ time: now(), level: "WARN", message: "请先输入 CustomerCode" });
    return;
  }
  busy.value = true;
  creditData.value = [];
  try {
    const res = await invoke<any>("query_credit", {
      customerCode: customerCode.value.trim(),
      connIdx: currentSession().connIdx,
      sessIdx: currentSession().sessIdx,
    });
    creditData.value = [
      { name: creditItems[0], value: fmt(res.total_credit_limit) },
      { name: creditItems[1], value: fmt(res.open_orders) },
      { name: creditItems[2], value: fmt(res.open_delivery) },
      { name: creditItems[3], value: fmt(res.total_receivables) },
      { name: creditItems[4], value: fmt(res.available_delivery), bold: true },
    ];
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
    appendLog({ time: now(), level: "WARN", message: "请至少输入一个 OrderId" });
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
    orderSummary.value = `合计（已查 ${count} 个 OrderId）`;
  }
  orderSummary.value = `合计（共 ${count} 个 OrderId）：${fmt(total)}`;
  busy.value = false;
}

// ---------- 交货单修改 ----------
async function runDelivery() {
  if (!deliveryId.value.trim()) {
    appendLog({ time: now(), level: "WARN", message: "请先输入 DeliveryId" });
    return;
  }
  busy.value = true;
  deliveryResult.value = "执行中，请稍候...（SAP 界面会被自动操作，请勿手工点击）";
  try {
    const res = await invoke<string[]>("modify_delivery", {
      orderNo: deliveryId.value.trim(),
      isSave: false,
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
  for (const item of creditData.value) {
    lines.push(`${item.name}\t${item.value}`);
  }
  return lines.join("\n");
}

function orderTableText(): string {
  const lines = ["OrderId\t金额 / 返回值"];
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
  // 监听后端日志事件
  await listen("log://entry", (event) => {
    const payload = event.payload as { time: string; level: string; message: string };
    appendLog(payload);
  });

  // 获取日志目录
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
    <!-- 顶部栏：会话 + 隐藏日志按钮 -->
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

    <!-- 主体：左侧功能区 + 右侧日志 -->
    <div class="body">
      <div class="functions">
        <!-- ① 客户信用信息查询 -->
        <div class="card">
          <div class="card-title">① 客户信用信息查询</div>
          <div class="card-body credit-body">
            <div class="input-col">
              <label>CustomerCode</label>
              <input v-model="customerCode" placeholder="例如 0070028763" @keyup.enter="runCredit" />
            </div>
            <button class="btn primary" :disabled="busy" @click="runCredit">查询信用</button>
            <div class="table-col">
              <table class="data-table">
                <thead>
                  <tr><th>项目</th><th>数值</th></tr>
                </thead>
                <tbody>
                  <tr v-for="item in creditData" :key="item.name">
                    <td :class="{ bold: item.bold }">{{ item.name }}</td>
                    <td :class="{ bold: item.bold, red: item.bold }">{{ item.value }}</td>
                  </tr>
                  <tr v-if="creditData.length === 0">
                    <td colspan="2" class="empty">（未查询）</td>
                  </tr>
                </tbody>
              </table>
              <button class="btn small" :disabled="creditData.length === 0" @click="copyText(creditTableText(), '信用表格')">复制表格</button>
            </div>
          </div>
        </div>

        <!-- ② 订单金额批量查询 -->
        <div class="card">
          <div class="card-title">② 订单金额批量查询</div>
          <div class="card-body order-body">
            <div class="input-col">
              <label>OrderId 列表（每行一个）</label>
              <textarea v-model="orderIds" placeholder="40844941&#10;41129429&#10;9071842" rows="6"></textarea>
              <label class="checkbox">
                <input type="checkbox" v-model="onlyHead" />
                仅取抬头净值(onlyHead)
              </label>
            </div>
            <button class="btn primary" :disabled="busy" @click="runOrders">批量查询金额</button>
            <div class="table-col">
              <table class="data-table">
                <thead>
                  <tr><th>OrderId</th><th>金额 / 返回值</th></tr>
                </thead>
                <tbody>
                  <tr v-for="(row, i) in orderRows" :key="i">
                    <td>{{ row.orderId }}</td>
                    <td>{{ row.amount }}</td>
                  </tr>
                  <tr v-if="orderRows.length > 0">
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

        <!-- ③ 修改外向交货单 -->
        <div class="card">
          <div class="card-title">③ 修改外向交货单输出 ZPZ3</div>
          <div class="card-body delivery-body">
            <div class="input-col">
              <label>DeliveryId (交货单号)</label>
              <input v-model="deliveryId" placeholder="例如 40844941" @keyup.enter="runDelivery" />
            </div>
            <button class="btn primary" :disabled="busy" @click="runDelivery">修改交货单</button>
            <div class="result-box">
              <div class="result-title">执行结果</div>
              <pre class="result-text">{{ deliveryResult || "函数执行结果会显示在这里" }}</pre>
            </div>
          </div>
        </div>
      </div>

      <!-- 右侧日志面板 -->
      <div v-if="showLog" class="log-panel">
        <div class="log-header">
          <span>运行日志</span>
          <span class="log-dir" v-if="logDir">日志目录: {{ logDir }}</span>
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
  --bg: #f5f6f8;
  --card-bg: #ffffff;
  --border: #c8ced6;
  --text: #1f2933;
  --muted: #6b7280;
  --primary: #1f4e79;
  --red: #b03030;
  --green: #1e7a3a;
  --log-bg: #1e1e1e;
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

/* 顶部栏 */
.topbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 14px;
  background: #fff;
  border-bottom: 1px solid var(--border);
}

.session-row {
  display: flex;
  align-items: center;
  gap: 8px;
  flex: 1;
}

.label {
  font-weight: 600;
  color: var(--muted);
}

.session-select {
  min-width: 420px;
  padding: 5px 8px;
  border: 1px solid var(--border);
  border-radius: 4px;
  background: #fff;
}

.conn-status {
  font-weight: 600;
  padding: 2px 10px;
  border-radius: 10px;
}
.conn-status.ok {
  color: var(--green);
}
.conn-status.bad {
  color: var(--red);
}

/* 按钮 */
.btn {
  padding: 6px 12px;
  border: 1px solid #b9c2cc;
  border-radius: 5px;
  background: #f2f5f9;
  cursor: pointer;
  color: var(--text);
  font-size: 13px;
}
.btn:hover {
  background: #e3ebf5;
}
.btn:disabled {
  color: #9aa2ab;
  background: #f6f7f8;
  cursor: not-allowed;
}
.btn.primary {
  min-width: 150px;
  height: 46px;
  font-weight: 600;
  background: #eaf1f9;
  border-color: #9bb4d0;
}
.btn.small {
  padding: 4px 10px;
  font-size: 12px;
}

/* 主体布局 */
.body {
  display: flex;
  flex: 1;
  overflow: hidden;
}

.functions {
  flex: 1;
  overflow-y: auto;
  padding: 14px;
  display: flex;
  flex-direction: column;
  gap: 14px;
}

/* 卡片 */
.card {
  background: var(--card-bg);
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 12px;
}

.card-title {
  font-weight: 700;
  color: var(--primary);
  margin-bottom: 10px;
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
  width: 220px;
}

.input-col label {
  font-weight: 600;
  color: var(--muted);
}

.input-col input,
.input-col textarea {
  width: 100%;
  padding: 6px 8px;
  border: 1px solid var(--border);
  border-radius: 4px;
  font-size: 13px;
  font-family: inherit;
}

.checkbox {
  display: flex;
  align-items: center;
  gap: 6px;
  font-weight: 400;
  color: var(--text);
  cursor: pointer;
}

.table-col {
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.data-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 13px;
}

.data-table th,
.data-table td {
  border: 1px solid #d6dbe1;
  padding: 5px 8px;
  text-align: left;
}

.data-table th {
  background: #eef2f7;
  font-weight: 600;
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

/* 交货单结果 */
.result-box {
  flex: 1;
  border: 1px solid var(--border);
  border-radius: 4px;
  min-height: 80px;
}

.result-title {
  font-weight: 600;
  padding: 4px 8px;
  background: #eef2f7;
  border-bottom: 1px solid var(--border);
  font-size: 12px;
  color: var(--muted);
}

.result-text {
  padding: 8px;
  white-space: pre-wrap;
  word-break: break-all;
  font-family: inherit;
  font-size: 13px;
  min-height: 50px;
}

/* 右侧日志面板 */
.log-panel {
  width: 360px;
  min-width: 280px;
  border-left: 1px solid var(--border);
  background: var(--log-bg);
  display: flex;
  flex-direction: column;
}

.log-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 12px;
  background: #2a2a2a;
  color: #e0e0e0;
  font-weight: 600;
  border-bottom: 1px solid #3a3a3a;
}

.log-dir {
  font-size: 10px;
  color: #888;
  font-weight: 400;
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
