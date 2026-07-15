const { invoke, Channel } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

// ── i18n ──
let lang = {};
let currentLang = "en";

async function loadLang(code) {
  const resp = await fetch(`lang/${code}.json`);
  lang = await resp.json();
  currentLang = code;
  document.querySelectorAll("[data-i18n]").forEach((el) => {
    const key = el.dataset.i18n;
    if (lang[key]) el.textContent = lang[key];
  });
}

function t(key) {
  return lang[key] || key;
}

document.addEventListener("contextmenu", (e) => e.preventDefault());
document.addEventListener("keydown", (e) => { if (e.key === "F12") e.preventDefault(); });

// Window controls
document.querySelector(".top-backdrop").style.pointerEvents = "auto";
document.querySelector(".top-backdrop").addEventListener("mousedown", (e) => {
  if (e.button === 0 && !e.target.closest(".win-btn") && !e.target.closest(".status-indicator")) {
    invoke("plugin:window|start_dragging", { label: "main" });
  }
});
document.getElementById("minimizeBtn").addEventListener("click", async () => {
  invoke("plugin:window|hide", { label: "main" });
  try {
    await invoke("plugin:notification|request_permission");
    await invoke("plugin:notification|notify", {
      options: {
        title: "SpoitableHRS",
        body: t("notification.minimized") || "Minimized to system tray. Click the tray icon to restore.",
      }
    });
  } catch (e) {
    console.error("Notification failed:", e);
  }
});
document.getElementById("closeBtn").addEventListener("click", () => {
  invoke("plugin:window|close", { label: "main" });
});

// ── State ──
let isConnected = false;
let beatTimeout = null;
let connectedDevice = null;

// ── Elements ──
const app = document.querySelector(".app");
const bpmEl = document.getElementById("bpm");
const heartEl = document.getElementById("heart");
const hrZoneEl = document.getElementById("hrZone");
const statusIndicator = document.getElementById("statusIndicator");
const statusDot = statusIndicator.querySelector(".status-dot");
const statusLabel = statusIndicator.querySelector(".status-label");
const monitorConnected = document.getElementById("monitorConnected");
const monitorDisconnected = document.getElementById("monitorDisconnected");
const scanCircle = document.getElementById("scanCircle");
const scanSpinner = document.getElementById("scanSpinner");
const scanLabel = document.getElementById("scanLabel");
const deviceModal = document.getElementById("deviceModal");
const deviceListBody = document.getElementById("deviceListBody");
const modalCloseBtn = document.getElementById("modalCloseBtn");
const logContainer = document.getElementById("logContainer");
const hrCanvas = document.getElementById("hrCanvas");

// ── HR Graph ──
const MAX_POINTS = 120;
const hrHistory = [];

function initCanvas() {
  const rect = hrCanvas.parentElement.getBoundingClientRect();
  hrCanvas.width = rect.width * devicePixelRatio;
  hrCanvas.height = rect.height * devicePixelRatio;
}

function drawGraph() {
  const ctx = hrCanvas.getContext("2d");
  const w = hrCanvas.width;
  const h = hrCanvas.height;
  ctx.clearRect(0, 0, w, h);
  if (hrHistory.length < 2) return;

  const data = hrHistory;
  const mn = Math.min(...data) - 3;
  const mx = Math.max(...data) + 3;
  const rng = mx - mn;
  const step = w / (MAX_POINTS - 1);
  const offset = MAX_POINTS - data.length;

  // Fill
  const fillGrad = ctx.createLinearGradient(0, 0, 0, h);
  fillGrad.addColorStop(0, "rgba(231,76,111,0.06)");
  fillGrad.addColorStop(1, "rgba(58,134,255,0)");

  // Line gradient
  const lineGrad = ctx.createLinearGradient(0, 0, 0, h);
  lineGrad.addColorStop(0, "#ff5555");
  lineGrad.addColorStop(0.5, "#e74c6f");
  lineGrad.addColorStop(1, "#3a86ff");

  const pts = data.map((v, i) => [(offset + i) * step, h - ((v - mn) / rng) * h]);

  // Smooth curve
  ctx.beginPath();
  ctx.moveTo(pts[0][0], pts[0][1]);
  for (let i = 0; i < pts.length - 1; i++) {
    const cx = (pts[i][0] + pts[i + 1][0]) / 2;
    ctx.bezierCurveTo(cx, pts[i][1], cx, pts[i + 1][1], pts[i + 1][0], pts[i + 1][1]);
  }
  ctx.strokeStyle = lineGrad;
  ctx.lineWidth = 1.5 * devicePixelRatio;
  ctx.lineJoin = "round";
  ctx.stroke();

  // Fill under
  ctx.lineTo(pts[pts.length - 1][0], h);
  ctx.lineTo(pts[0][0], h);
  ctx.closePath();
  ctx.fillStyle = fillGrad;
  ctx.fill();
}

// ── HR Zones ──
function getZone(hr) {
  if (hr >= 140) return { name: t("zone.hard"), color: "#e74c3c" };
  if (hr >= 120) return { name: t("zone.moderate"), color: "#f39c12" };
  if (hr >= 100) return { name: t("zone.light"), color: "#3a86ff" };
  return { name: t("zone.rest"), color: "#2ecc71" };
}

// ── Status ──
function setStatus(state, label, color) {
  statusDot.style.background = color;
  statusLabel.style.color = color;
  statusLabel.textContent = label;
  statusDot.classList.toggle("pulse", state === "searching" || state === "connecting");
  statusIndicator.classList.toggle("clickable", state === "connected");
}

function updateConnectionUI() {
  if (isConnected) {
    app.classList.add("connected");
    monitorConnected.classList.remove("hidden");
    monitorDisconnected.classList.add("hidden");
    heartEl.classList.add("beating");
    setStatus("connected", t("status.connected"), "#2ecc71");
    if (connectedDevice) {
      document.getElementById("connectedDeviceName").textContent = connectedDevice.name;
      document.getElementById("connectedDeviceId").textContent = "";
    }
    initCanvas();
  } else {
    app.classList.remove("connected");
    monitorConnected.classList.add("hidden");
    monitorDisconnected.classList.remove("hidden");
    heartEl.classList.remove("beating");
    scanCircle.classList.remove("hidden");
    scanSpinner.classList.add("hidden");
    scanLabel.textContent = "";
    setStatus("disconnected", t("status.disconnected"), "#e74c3c");
    bpmEl.textContent = "";
    hrZoneEl.textContent = "";
    hrHistory.length = 0;
  }
}

// ── Status click → disconnect ──
statusIndicator.addEventListener("click", async () => {
  if (!isConnected) return;
  addLog("Disconnecting...");
  await invoke("disconnect_device");
  isConnected = false;
  connectedDevice = null;
  updateConnectionUI();
  addLog("Disconnected");
});

// ── Scan & Connect ──
scanCircle.addEventListener("click", async () => {
  scanCircle.classList.add("hidden");
  scanSpinner.classList.remove("hidden");
  scanLabel.textContent = t("monitor.scanning");
  scanLabel.style.color = "#999";
  setStatus("searching", t("status.searching"), "#3a86ff");
  addLog("Starting BLE scan...");

  try {
    const devices = await invoke("scan_devices");
    addLog(`Scan complete: ${devices.length} device(s) found`);
    devices.forEach((d) => addLog(`  ${d.name} (${d.id})`));

    if (devices.length === 0) {
      scanSpinner.classList.add("hidden");
      scanCircle.classList.remove("hidden");
      scanLabel.textContent = t("monitor.noDevices");
      scanLabel.style.color = "#e74c3c";
      setStatus("disconnected", t("status.disconnected"), "#e74c3c");
      addLog("No devices found", "warn");
      return;
    }

    // Show modal
    scanSpinner.classList.add("hidden");
    scanLabel.textContent = "";
    setStatus("disconnected", t("status.deviceFound"), "#3a86ff");
    deviceModal.classList.add("active");
    deviceListBody.innerHTML = "";

    devices.forEach((d) => {
      const item = document.createElement("div");
      item.className = "device-item";
      item.innerHTML = `
        <div><div class="device-name">${d.name}</div><div class="device-id">${d.id}</div></div>
        <span class="device-arrow">&#x203A;</span>
      `;
      item.addEventListener("click", () => connectToDevice(d));
      deviceListBody.appendChild(item);
    });
  } catch (e) {
    scanSpinner.classList.add("hidden");
    scanCircle.classList.remove("hidden");
    scanLabel.textContent = "";
    setStatus("disconnected", t("status.disconnected"), "#e74c3c");
    addLog(`Scan failed: ${e}`, "error");
  }
});

async function connectToDevice(device) {
  deviceModal.classList.remove("active");
  connectedDevice = device;

  scanCircle.classList.add("hidden");
  scanSpinner.classList.remove("hidden");
  scanLabel.textContent = t("monitor.connecting");
  setStatus("connecting", t("status.connecting"), "#3a86ff");
  addLog(`Connecting to ${device.id}...`);

  try {
    await invoke("connect_device", { deviceId: device.id });
  } catch (e) {
    scanSpinner.classList.add("hidden");
    scanCircle.classList.remove("hidden");
    scanLabel.textContent = "";
    setStatus("disconnected", t("status.disconnected"), "#e74c3c");
    addLog(`Connection failed: ${e}`, "error");
  }
}

// Modal close
modalCloseBtn.addEventListener("click", () => {
  deviceModal.classList.remove("active");
  scanCircle.classList.remove("hidden");
  setStatus("disconnected", t("status.disconnected"), "#e74c3c");
});
deviceModal.addEventListener("click", (e) => {
  if (e.target === deviceModal) {
    deviceModal.classList.remove("active");
    scanCircle.classList.remove("hidden");
    setStatus("disconnected", t("status.disconnected"), "#e74c3c");
  }
});

// ── Events ──
let graphDrawInterval = null;
let latestHr = 0;

function startGraphLoop(intervalMs) {
  if (graphDrawInterval) clearInterval(graphDrawInterval);
  graphDrawInterval = setInterval(() => {
    if (isConnected && latestHr > 0) {
      hrHistory.push(latestHr);
      if (hrHistory.length > MAX_POINTS) hrHistory.shift();
      drawGraph();
    }
  }, intervalMs);
}

listen("heart-rate-update", (event) => {
  latestHr = event.payload;
  bpmEl.textContent = latestHr;
  const zone = getZone(latestHr);
  hrZoneEl.textContent = zone.name;
  hrZoneEl.style.color = zone.color;
});

listen("connection-changed", (event) => {
  isConnected = event.payload;
  updateConnectionUI();
  if (!isConnected) {
    document.getElementById("modeBanner").classList.add("hidden");
    document.getElementById("batteryLevel").classList.add("hidden");
  }
});

listen("connection-mode", (event) => {
  const mode = event.payload;
  const banner = document.getElementById("modeBanner");
  const text = document.getElementById("modeBannerText");
  banner.classList.remove("hidden", "gatt", "broadcast", "unsupported");
  if (mode === "gatt") {
    banner.classList.add("gatt");
    text.textContent = t("mode.gatt");
  } else if (mode === "unsupported") {
    banner.classList.add("unsupported");
    text.textContent = t("mode.unsupported");
  } else {
    banner.classList.add("broadcast");
    text.textContent = t("mode.broadcast");
  }
});

listen("battery-update", (event) => {
  const level = event.payload;
  const el = document.getElementById("batteryLevel");
  el.textContent = `🔋 ${level}%`;
  el.classList.remove("hidden");
  if (level <= 20) {
    el.style.color = "#e74c3c";
  } else if (level <= 50) {
    el.style.color = "#f39c12";
  } else {
    el.style.color = "#2ecc71";
  }
});

listen("ble-log", (event) => {
  const { message, level } = event.payload;
  addLog(message, level);
});

// ── Sidebar ──
const sidebar = document.getElementById("sidebar");
const sidebarHint = document.getElementById("sidebarHint");
const sidebarTrigger = document.getElementById("sidebarTrigger");
let sidebarCloseTimer = null;

function openSidebar() {
  clearTimeout(sidebarCloseTimer);
  sidebar.classList.add("open");
  sidebarHint.classList.add("hidden");
}
function startCloseSidebar() {
  sidebarCloseTimer = setTimeout(() => {
    sidebar.classList.remove("open");
    sidebarHint.classList.remove("hidden");
  }, 400);
}

sidebarTrigger.addEventListener("mouseenter", openSidebar);
sidebarTrigger.addEventListener("mouseleave", startCloseSidebar);
sidebar.addEventListener("mouseenter", () => clearTimeout(sidebarCloseTimer));
sidebar.addEventListener("mouseleave", startCloseSidebar);

document.querySelectorAll(".sidebar-icon").forEach((icon) => {
  icon.addEventListener("click", () => {
    const viewName = icon.dataset.view;
    // Stats is a collapsible drawer, not a swappable view — toggling it is
    // independent of which left-side view is active.
    if (viewName === "stats") {
      toggleStats();
      return;
    }
    // Switching the left view keeps the stats drawer open; only clear the
    // active state of the other (non-stats) view icons.
    document.querySelectorAll(".sidebar-icon").forEach((i) => {
      if (i.dataset.view !== "stats") i.classList.remove("active");
    });
    document.querySelectorAll(".view").forEach((v) => v.classList.remove("active"));
    icon.classList.add("active");
    document.getElementById(`view-${viewName}`).classList.add("active");
    if (viewName === "monitor" && isConnected) initCanvas();
  });
});

// ── Toggles ──
document.querySelectorAll(".toggle-switch").forEach((toggle) => {
  toggle.addEventListener("click", () => {
    const checked = toggle.dataset.checked === "true";
    toggle.dataset.checked = (!checked).toString();
  });
});

// ── Stats drawer ──
const statsIcon = document.querySelector('.sidebar-icon[data-view="stats"]');
const statsCanvas = document.getElementById("statsCanvas");
const statsDateInput = document.getElementById("statsDate");
const statMinEl = document.getElementById("statMin");
const statMaxEl = document.getElementById("statMax");
const statAvgEl = document.getElementById("statAvg");
const statRangeLabel = document.getElementById("statRangeLabel");

let statsOpen = false;
let statsData = []; // [{ t, bpm }]
let statsFrom = 0;
let statsTo = 0;
let selStart = null; // snapped selection start (epoch ms)
let selEnd = null; // snapped selection end (epoch ms)
let rawStart = null; // un-snapped drag anchor
let rawEnd = null;
let statsDragging = false;
let statsMoved = false;
let statsDownX = 0;
let shiftHeld = false;

const STATS_PAD = { l: 34, r: 10, t: 12, b: 20 };
const SNAP_COARSE_MS = 15 * 60 * 1000; // default snap: 15 min
const SNAP_FINE_MS = 1 * 60 * 1000; // Shift held: 1 min
const HOUR_MS = 60 * 60 * 1000;
const dprOf = () => window.devicePixelRatio || 1;

function snapFloor(tms, unit) {
  return statsFrom + Math.floor((tms - statsFrom) / unit) * unit;
}
function snapCeil(tms, unit) {
  return statsFrom + Math.ceil((tms - statsFrom) / unit) * unit;
}
// Snap the drag range outward to clean unit boundaries so it covers the drag.
function applyStatsSnap() {
  if (rawStart == null || rawEnd == null) {
    selStart = null;
    selEnd = null;
    return;
  }
  const unit = shiftHeld ? SNAP_FINE_MS : SNAP_COARSE_MS;
  const lo = Math.min(rawStart, rawEnd);
  const hi = Math.max(rawStart, rawEnd);
  selStart = Math.max(statsFrom, snapFloor(lo, unit));
  selEnd = Math.min(statsTo, snapCeil(hi, unit));
  if (selEnd <= selStart) selEnd = Math.min(statsTo, selStart + unit);
}

function fmtDateInput(d) {
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
}

function setStatsDateToday() {
  statsDateInput.value = fmtDateInput(new Date());
}

function currentDayRange() {
  const parts = statsDateInput.value.split("-").map(Number);
  if (parts.length !== 3 || parts.some(Number.isNaN)) {
    const d = new Date();
    d.setHours(0, 0, 0, 0);
    return [d.getTime(), d.getTime() + 86400000 - 1];
  }
  const [y, m, d] = parts;
  const from = new Date(y, m - 1, d, 0, 0, 0, 0).getTime();
  return [from, from + 86400000 - 1];
}

function fmtClock(ms) {
  return new Date(ms).toLocaleTimeString("ja-JP", { hour: "2-digit", minute: "2-digit", hour12: false });
}

async function toggleStats(force) {
  const next = force !== undefined ? force : !statsOpen;
  if (next === statsOpen) return;
  statsOpen = next;
  app.classList.toggle("stats-open", statsOpen);
  statsIcon.classList.toggle("active", statsOpen);
  try {
    await invoke("set_stats_expanded", { expanded: statsOpen });
  } catch (e) {
    console.error("set_stats_expanded failed:", e);
  }
  if (statsOpen) {
    if (!statsDateInput.value) setStatsDateToday();
    // wait for the webview to finish resizing before measuring the canvas
    setTimeout(loadStats, 120);
  }
}

async function loadStats() {
  [statsFrom, statsTo] = currentDayRange();
  selStart = null;
  selEnd = null;
  rawStart = null;
  rawEnd = null;
  statsMoved = false;
  try {
    statsData = (await invoke("read_hr_records", { from: statsFrom, to: statsTo })) || [];
  } catch (e) {
    console.error("read_hr_records failed:", e);
    statsData = [];
  }
  drawStats();
  updateStatTiles();
}

function computeStats(a, b) {
  let mn = Infinity;
  let mx = -Infinity;
  let sum = 0;
  let n = 0;
  for (const p of statsData) {
    if (p.t < a || p.t > b) continue;
    if (p.bpm < mn) mn = p.bpm;
    if (p.bpm > mx) mx = p.bpm;
    sum += p.bpm;
    n++;
  }
  if (!n) return null;
  return { min: mn, max: mx, avg: Math.round(sum / n), count: n };
}

function updateStatTiles() {
  const useSel = selStart != null && selEnd != null && Math.abs(selEnd - selStart) > 0;
  const [a, b] = useSel
    ? [Math.min(selStart, selEnd), Math.max(selStart, selEnd)]
    : [statsFrom, statsTo];
  const s = computeStats(a, b);
  if (!s) {
    statMinEl.textContent = "--";
    statMaxEl.textContent = "--";
    statAvgEl.textContent = "--";
    statRangeLabel.textContent = t("stats.noData");
    return;
  }
  statMinEl.textContent = s.min;
  statMaxEl.textContent = s.max;
  statAvgEl.textContent = s.avg;
  statRangeLabel.textContent = useSel ? `${fmtClock(a)} – ${fmtClock(b)}` : t("stats.fullDay");
}

function initStatsCanvas() {
  const rect = statsCanvas.parentElement.getBoundingClientRect();
  const dpr = dprOf();
  statsCanvas.width = Math.max(1, Math.round(rect.width * dpr));
  statsCanvas.height = Math.max(1, Math.round(rect.height * dpr));
}

function niceStep(range, target) {
  const raw = range / target;
  const pow = Math.pow(10, Math.floor(Math.log10(raw)));
  const norm = raw / pow;
  let step;
  if (norm < 1.5) step = 1;
  else if (norm < 3) step = 2;
  else if (norm < 7) step = 5;
  else step = 10;
  return Math.max(1, step * pow);
}

function drawStats() {
  initStatsCanvas();
  const ctx = statsCanvas.getContext("2d");
  const dpr = dprOf();
  const w = statsCanvas.width;
  const h = statsCanvas.height;
  ctx.clearRect(0, 0, w, h);

  const padL = STATS_PAD.l * dpr;
  const padR = STATS_PAD.r * dpr;
  const padT = STATS_PAD.t * dpr;
  const padB = STATS_PAD.b * dpr;
  const plotW = w - padL - padR;
  const plotH = h - padT - padB;

  if (!statsData.length) {
    ctx.fillStyle = "#555";
    ctx.font = `${13 * dpr}px "Segoe UI", system-ui, sans-serif`;
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    ctx.fillText(t("stats.noData"), w / 2, h / 2);
    return;
  }

  let mn = Infinity;
  let mx = -Infinity;
  for (const p of statsData) {
    if (p.bpm < mn) mn = p.bpm;
    if (p.bpm > mx) mx = p.bpm;
  }
  mn = Math.floor(mn - 5);
  mx = Math.ceil(mx + 5);
  if (mx - mn < 10) mx = mn + 10;

  const span = statsTo - statsFrom || 1;
  const xOf = (tms) => padL + ((tms - statsFrom) / span) * plotW;
  const yOf = (v) => padT + (1 - (v - mn) / (mx - mn)) * plotH;

  // horizontal gridlines + y labels
  ctx.lineWidth = 1;
  ctx.font = `${10 * dpr}px "Segoe UI", system-ui, sans-serif`;
  ctx.textAlign = "right";
  ctx.textBaseline = "middle";
  const yStep = niceStep(mx - mn, 4);
  for (let v = Math.ceil(mn / yStep) * yStep; v <= mx; v += yStep) {
    const y = yOf(v);
    ctx.strokeStyle = "rgba(255,255,255,0.05)";
    ctx.beginPath();
    ctx.moveTo(padL, y);
    ctx.lineTo(w - padR, y);
    ctx.stroke();
    ctx.fillStyle = "#666";
    ctx.fillText(String(v), padL - 5 * dpr, y);
  }

  // vertical hourly gridlines + hour labels
  const hourCssPx = plotW / dpr / 24;
  const labelStep = hourCssPx >= 20 ? 1 : hourCssPx >= 12 ? 2 : 3;
  ctx.textAlign = "center";
  ctx.textBaseline = "top";
  const xLabelY = padT + plotH + 4 * dpr;
  for (let hh = 0; hh <= 24; hh++) {
    const x = xOf(statsFrom + hh * HOUR_MS);
    ctx.strokeStyle = "rgba(255,255,255,0.04)";
    ctx.beginPath();
    ctx.moveTo(x, padT);
    ctx.lineTo(x, padT + plotH);
    ctx.stroke();
    if (hh < 24 && hh % labelStep === 0) {
      ctx.fillStyle = "#666";
      ctx.fillText(String(hh), x, xLabelY);
    }
  }

  // selection band
  if (selStart != null && selEnd != null) {
    const x0 = xOf(Math.min(selStart, selEnd));
    const x1 = xOf(Math.max(selStart, selEnd));
    ctx.fillStyle = "rgba(58,134,255,0.15)";
    ctx.fillRect(x0, padT, Math.max(1, x1 - x0), plotH);
    ctx.strokeStyle = "rgba(58,134,255,0.5)";
    ctx.beginPath();
    ctx.moveTo(x0, padT);
    ctx.lineTo(x0, padT + plotH);
    ctx.moveTo(x1, padT);
    ctx.lineTo(x1, padT + plotH);
    ctx.stroke();
  }

  // HR line, split across gaps > 5 min
  ctx.strokeStyle = "#e74c6f";
  ctx.lineWidth = 1.5 * dpr;
  ctx.lineJoin = "round";
  ctx.beginPath();
  const GAP = 5 * 60 * 1000;
  let started = false;
  for (let i = 0; i < statsData.length; i++) {
    const p = statsData[i];
    const x = xOf(p.t);
    const y = yOf(p.bpm);
    if (!started || (i > 0 && p.t - statsData[i - 1].t > GAP)) {
      ctx.moveTo(x, y);
      started = true;
    } else {
      ctx.lineTo(x, y);
    }
  }
  ctx.stroke();
}

function statsXToTime(clientX) {
  const rect = statsCanvas.getBoundingClientRect();
  const plotCssW = rect.width - STATS_PAD.l - STATS_PAD.r;
  let frac = (clientX - rect.left - STATS_PAD.l) / plotCssW;
  frac = Math.max(0, Math.min(1, frac));
  return Math.round(statsFrom + frac * (statsTo - statsFrom));
}

statsCanvas.addEventListener("mousedown", (e) => {
  if (!statsData.length) return;
  statsDragging = true;
  statsMoved = false;
  statsDownX = e.clientX;
  shiftHeld = e.shiftKey;
  rawStart = statsXToTime(e.clientX);
  rawEnd = rawStart;
  selStart = null; // no band until an actual drag
  selEnd = null;
  drawStats();
});
window.addEventListener("mousemove", (e) => {
  if (!statsDragging) return;
  shiftHeld = e.shiftKey;
  rawEnd = statsXToTime(e.clientX);
  if (Math.abs(e.clientX - statsDownX) > 3) statsMoved = true;
  if (statsMoved) applyStatsSnap();
  drawStats();
  updateStatTiles();
});
window.addEventListener("mouseup", () => {
  if (!statsDragging) return;
  statsDragging = false;
  if (!statsMoved) {
    // a plain click clears any selection
    rawStart = null;
    rawEnd = null;
    selStart = null;
    selEnd = null;
  } else {
    applyStatsSnap();
  }
  drawStats();
  updateStatTiles();
});
// Holding/releasing Shift mid-drag switches snap granularity live
window.addEventListener("keydown", (e) => {
  if (e.key === "Shift" && statsDragging && !shiftHeld) {
    shiftHeld = true;
    if (statsMoved) applyStatsSnap();
    drawStats();
    updateStatTiles();
  }
});
window.addEventListener("keyup", (e) => {
  if (e.key === "Shift" && statsDragging) {
    shiftHeld = false;
    if (statsMoved) applyStatsSnap();
    drawStats();
    updateStatTiles();
  }
});

function shiftStatsDay(delta) {
  const parts = statsDateInput.value.split("-").map(Number);
  const base = parts.length === 3 && !parts.some(Number.isNaN)
    ? new Date(parts[0], parts[1] - 1, parts[2])
    : new Date();
  base.setDate(base.getDate() + delta);
  statsDateInput.value = fmtDateInput(base);
  loadStats();
}

statsDateInput.addEventListener("change", loadStats);
document.getElementById("statsPrev").addEventListener("click", () => shiftStatsDay(-1));
document.getElementById("statsNext").addEventListener("click", () => shiftStatsDay(1));
document.getElementById("statsToday").addEventListener("click", () => {
  setStatsDateToday();
  loadStats();
});

window.addEventListener("resize", () => {
  if (statsOpen) drawStats();
});

// ── OSC Settings ──
const oscToggle = document.getElementById("oscToggle");
const oscPort = document.getElementById("oscPort");
const saveParamsBtn = document.getElementById("saveParamsBtn");

const PARAM_FIELDS = [
  "hr", "ones_hr", "tens_hr", "hundreds_hr",
  "is_hr_connected", "is_hr_active", "is_hr_beat",
  "hr_percent", "full_hr_percent",
];

oscToggle.addEventListener("click", () => {
  const enabled = oscToggle.dataset.checked === "true";
  invoke("set_osc_enabled", { enabled });
  addLog(`OSC output ${enabled ? "enabled" : "disabled"}`);
});

oscPort.addEventListener("change", () => {
  const port = parseInt(oscPort.value, 10);
  if (port > 0 && port <= 65535) {
    invoke("set_osc_port", { port });
    addLog(`OSC port set to ${port}`);
  }
});

saveParamsBtn.addEventListener("click", async () => {
  const params = {};
  for (const field of PARAM_FIELDS) {
    params[field] = document.getElementById(`param-${field}`).value;
  }
  await invoke("set_osc_params", { params });
  addLog("OSC parameter names saved");
});

async function loadOscParams() {
  try {
    const params = await invoke("get_osc_params");
    for (const field of PARAM_FIELDS) {
      const el = document.getElementById(`param-${field}`);
      if (el && params[field] !== undefined) el.value = params[field];
    }
  } catch (e) { console.error("Failed to load OSC params:", e); }
}

// ── Settings toggles ──
document.getElementById("alwaysOnTopToggle").addEventListener("click", () => {
  const toggle = document.getElementById("alwaysOnTopToggle");
  const enabled = toggle.dataset.checked === "true";
  invoke("set_always_on_top", { enabled });
  invoke("plugin:window|set_always_on_top", { label: "main", value: enabled });
  addLog(`Always on top: ${enabled ? "on" : "off"}`);
});

document.getElementById("startMinToggle").addEventListener("click", () => {
  const toggle = document.getElementById("startMinToggle");
  const enabled = toggle.dataset.checked === "true";
  invoke("set_start_minimized", { enabled });
  addLog(`Start minimized: ${enabled ? "on" : "off"}`);
});

document.getElementById("graphInterval").addEventListener("change", () => {
  const val = parseInt(document.getElementById("graphInterval").value, 10);
  if (val >= 100 && val <= 5000) {
    invoke("set_graph_interval", { interval: val });
    startGraphLoop(val);
    addLog(`Graph interval: ${val}ms`);
  }
});

// ── Recording settings ──
document.getElementById("recordingToggle").addEventListener("click", () => {
  const enabled = document.getElementById("recordingToggle").dataset.checked === "true";
  invoke("set_recording_enabled", { enabled });
  addLog(`Recording: ${enabled ? "on" : "off"}`);
});

document.getElementById("recordInterval").addEventListener("change", () => {
  const val = parseInt(document.getElementById("recordInterval").value, 10);
  if (val >= 250 && val <= 60000) {
    invoke("set_record_interval", { interval: val });
    addLog(`Record interval: ${val}ms`);
  }
});

document.getElementById("flushInterval").addEventListener("change", () => {
  const val = parseInt(document.getElementById("flushInterval").value, 10);
  if (val >= 500 && val <= 300000) {
    invoke("set_flush_interval", { interval: val });
    addLog(`Flush interval: ${val}ms`);
  }
});

document.getElementById("openRecordsBtn").addEventListener("click", () => {
  invoke("open_records_dir");
});

document.getElementById("langSelect").addEventListener("change", async (e) => {
  const code = e.target.value;
  await loadLang(code);
  updateBtnText();
  if (isConnected) {
    setStatus("connected", t("status.connected"), "#2ecc71");
  } else {
    setStatus("disconnected", t("status.disconnected"), "#e74c3c");
  }
  invoke("set_language", { language: code });
  addLog(`Language: ${code}`);
});

// ── Log ──
function addLog(message, level = "info") {
  const entry = document.createElement("div");
  entry.className = `log-entry ${level}`;
  const now = new Date();
  const time = now.toLocaleTimeString("ja-JP", { hour12: false });
  entry.innerHTML = `<span class="time">[${time}]</span> ${message}`;
  logContainer.appendChild(entry);
  logContainer.scrollTop = logContainer.scrollHeight;
}

document.getElementById("clearLogBtn").addEventListener("click", () => {
  logContainer.innerHTML = "";
});

document.getElementById("copyLogBtn").addEventListener("click", () => {
  const text = logContainer.innerText;
  navigator.clipboard.writeText(text);
  addLog("Log copied to clipboard");
});

// ── Overlay tab ──
const OVERLAYS = [
  { name: "Pill Badge", file: "pill", desc: "Minimal · Corner placement", size: "400×112" },
  { name: "Glass Card", file: "glass", desc: "Liquid Glass · Mini graph", size: "600×260" },
  { name: "Neon Ring", file: "neon", desc: "Cyberpunk · Circular progress", size: "384×384" },
  { name: "Full Widget", file: "widget", desc: "Full info · Graph + Stats", size: "680×320" },
];

function renderOverlayList() {
  const list = document.getElementById("overlayList");
  const port = document.getElementById("wsPort").value || "9100";
  list.innerHTML = "";
  OVERLAYS.forEach((o) => {
    const card = document.createElement("div");
    card.className = "overlay-card";
    card.innerHTML = `
      <div class="overlay-preview">
        <iframe src="http://localhost:${port}/overlay/${o.file}" loading="lazy"></iframe>
      </div>
      <div class="overlay-info">
        <div>
          <div class="overlay-name">${o.name}</div>
          <div class="overlay-desc">${o.desc} · ${o.size}</div>
        </div>
        <div class="overlay-btns">
          <button class="overlay-btn" data-action="url" data-file="${o.file}">URL</button>
          <button class="overlay-btn accent" data-action="html" data-file="${o.file}">HTML</button>
        </div>
      </div>
    `;
    list.appendChild(card);
  });

  list.addEventListener("click", async (e) => {
    const btn = e.target.closest(".overlay-btn");
    if (!btn) return;
    const file = btn.dataset.file;
    const action = btn.dataset.action;
    const p = document.getElementById("wsPort").value || "9100";

    if (action === "url") {
      await navigator.clipboard.writeText(`http://localhost:${p}/overlay/${file}`);
    } else {
      try {
        const resp = await fetch(`http://localhost:${p}/overlay/${file}`);
        const html = await resp.text();
        await navigator.clipboard.writeText(html);
      } catch {
        await navigator.clipboard.writeText(`http://localhost:${p}/overlay/${file}`);
      }
    }
    btn.classList.add("copied");
    btn.textContent = "Copied!";
    setTimeout(() => { btn.classList.remove("copied"); btn.textContent = action === "url" ? "URL" : "HTML"; }, 1500);
  });
}

document.getElementById("wsToggle").addEventListener("click", () => {
  const enabled = document.getElementById("wsToggle").dataset.checked === "true";
  invoke("set_ws_enabled", { enabled });
  addLog(`WebSocket server ${enabled ? "enabled" : "disabled"}`);
});

document.getElementById("wsPort").addEventListener("change", () => {
  const port = parseInt(document.getElementById("wsPort").value, 10);
  if (port > 0 && port <= 65535) {
    invoke("set_ws_port", { port });
    addLog(`WebSocket port set to ${port}`);
    renderOverlayList();
  }
});

renderOverlayList();

// ── Init: load all saved settings ──
async function loadAllSettings() {
  await loadOscParams();

  const oscEnabled = await invoke("get_osc_enabled");
  oscToggle.dataset.checked = oscEnabled.toString();

  const oscPortVal = await invoke("get_osc_port");
  oscPort.value = oscPortVal;

  const wsEnabled = await invoke("get_ws_enabled");
  document.getElementById("wsToggle").dataset.checked = wsEnabled.toString();

  const wsPortVal = await invoke("get_ws_port");
  document.getElementById("wsPort").value = wsPortVal;

  const aot = await invoke("get_always_on_top");
  document.getElementById("alwaysOnTopToggle").dataset.checked = aot.toString();
  if (aot) invoke("plugin:window|set_always_on_top", { label: "main", value: true });

  const sm = await invoke("get_start_minimized");
  document.getElementById("startMinToggle").dataset.checked = sm.toString();

  const graphInt = await invoke("get_graph_interval");
  document.getElementById("graphInterval").value = graphInt;
  startGraphLoop(graphInt);

  const recEnabled = await invoke("get_recording_enabled");
  document.getElementById("recordingToggle").dataset.checked = recEnabled.toString();

  const recInt = await invoke("get_record_interval");
  document.getElementById("recordInterval").value = recInt;

  const flushInt = await invoke("get_flush_interval");
  document.getElementById("flushInterval").value = flushInt;

  const savedLang = await invoke("get_language");
  document.getElementById("langSelect").value = savedLang;
  await loadLang(savedLang);

  renderOverlayList();
}

loadAllSettings();
updateConnectionUI();

invoke("plugin:app|version").then(v => {
  document.getElementById("appVersion").textContent = `v${v}`;
  document.getElementById("titleVersion").textContent = `v${v}`;
}).catch(() => {});

addLog("SpoitableHRS initialized");

// ── Update check (manual) ──
let pendingUpdate = null;
let useTauriUpdater = false;
let updateState = "idle";

function updateBtnText() {
  const btn = document.getElementById("updateBtn");
  switch (updateState) {
    case "checking":
      btn.textContent = t("settings.checkingUpdate");
      btn.disabled = true;
      btn.classList.remove("update-ready");
      break;
    case "available":
      btn.textContent = `${t("settings.updateAvailable")}: v${pendingUpdate?.version}`;
      btn.disabled = false;
      btn.classList.add("update-ready");
      break;
    case "uptodate":
      btn.textContent = t("settings.upToDate");
      btn.disabled = true;
      btn.classList.remove("update-ready");
      break;
    case "updating":
      btn.textContent = t("settings.updating");
      btn.disabled = true;
      btn.classList.remove("update-ready");
      break;
    case "downloaded":
      btn.textContent = t("settings.downloadStarted");
      btn.disabled = true;
      btn.classList.remove("update-ready");
      break;
    case "failed":
      btn.textContent = t("settings.updateFailed");
      btn.disabled = false;
      btn.classList.remove("update-ready");
      break;
    default:
      btn.textContent = "";
      btn.disabled = true;
      break;
  }
}

async function checkForUpdates() {
  try {
    updateState = "checking";
    updateBtnText();
    addLog("Checking for updates...", "info");

    const result = await invoke("check_update");
    if (result && result.version) {
      const platform = result.platforms?.["windows-x86_64"] || {};
      addLog(`Update available: v${result.version}`, "info");
      pendingUpdate = {
        version: result.version,
        url: platform.url || result.url,
        signature: platform.signature || result.signature,
      };
      updateState = "available";
    } else {
      updateState = "uptodate";
      addLog("No updates available", "info");
    }
  } catch (e) {
    addLog(`Update check failed: ${e}`, "warn");
    updateState = "idle";
  }
  updateBtnText();
}

document.getElementById("updateBtn").addEventListener("click", async () => {
  if (!pendingUpdate || !pendingUpdate.url) return;
  updateState = "updating";
  updateBtnText();
  addLog("Downloading and installing update...", "info");

  try {
    await invoke("download_and_install_update", {
      url: pendingUpdate.url,
      signature: pendingUpdate.signature,
    });
  } catch (e) {
    addLog(`Update failed: ${e}`, "error");
    updateState = "failed";
    updateBtnText();
  }
});

checkForUpdates();
