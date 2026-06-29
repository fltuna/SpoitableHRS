const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

document.addEventListener("contextmenu", (e) => e.preventDefault());
document.addEventListener("keydown", (e) => { if (e.key === "F12") e.preventDefault(); });

// Window controls
document.querySelector(".top-backdrop").style.pointerEvents = "auto";
document.querySelector(".top-backdrop").addEventListener("mousedown", (e) => {
  if (e.button === 0 && !e.target.closest(".win-btn") && !e.target.closest(".status-indicator")) {
    invoke("plugin:window|start_dragging", { label: "main" });
  }
});
document.getElementById("minimizeBtn").addEventListener("click", () => {
  invoke("plugin:window|minimize", { label: "main" });
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
  if (hr >= 140) return { name: "Hard", color: "#e74c3c" };
  if (hr >= 120) return { name: "Moderate", color: "#f39c12" };
  if (hr >= 100) return { name: "Light", color: "#3a86ff" };
  return { name: "Rest Zone", color: "#2ecc71" };
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
    setStatus("connected", "Connected", "#2ecc71");
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
    setStatus("disconnected", "Disconnected", "#e74c3c");
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
  scanLabel.textContent = "Searching for devices...";
  scanLabel.style.color = "#999";
  setStatus("searching", "Searching...", "#3a86ff");
  addLog("Starting BLE scan...");

  try {
    const devices = await invoke("scan_devices");
    addLog(`Scan complete: ${devices.length} device(s) found`);
    devices.forEach((d) => addLog(`  ${d.name} (${d.id})`));

    if (devices.length === 0) {
      scanSpinner.classList.add("hidden");
      scanCircle.classList.remove("hidden");
      scanLabel.textContent = "No devices found";
      scanLabel.style.color = "#e74c3c";
      setStatus("disconnected", "Disconnected", "#e74c3c");
      addLog("No devices found", "warn");
      return;
    }

    // Show modal
    scanSpinner.classList.add("hidden");
    scanLabel.textContent = "";
    setStatus("disconnected", "Device Found", "#3a86ff");
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
    setStatus("disconnected", "Disconnected", "#e74c3c");
    addLog(`Scan failed: ${e}`, "error");
  }
});

async function connectToDevice(device) {
  deviceModal.classList.remove("active");
  connectedDevice = device;

  scanCircle.classList.add("hidden");
  scanSpinner.classList.remove("hidden");
  scanLabel.textContent = "Establishing connection...";
  setStatus("connecting", "Connecting...", "#3a86ff");
  addLog(`Connecting to ${device.id}...`);

  try {
    await invoke("connect_device", { deviceId: device.id });
  } catch (e) {
    scanSpinner.classList.add("hidden");
    scanCircle.classList.remove("hidden");
    scanLabel.textContent = "";
    setStatus("disconnected", "Disconnected", "#e74c3c");
    addLog(`Connection failed: ${e}`, "error");
  }
}

// Modal close
modalCloseBtn.addEventListener("click", () => {
  deviceModal.classList.remove("active");
  scanCircle.classList.remove("hidden");
  setStatus("disconnected", "Disconnected", "#e74c3c");
});
deviceModal.addEventListener("click", (e) => {
  if (e.target === deviceModal) {
    deviceModal.classList.remove("active");
    scanCircle.classList.remove("hidden");
    setStatus("disconnected", "Disconnected", "#e74c3c");
  }
});

// ── Events ──
listen("heart-rate-update", (event) => {
  const hr = event.payload;
  bpmEl.textContent = hr;
  const zone = getZone(hr);
  hrZoneEl.textContent = zone.name;
  hrZoneEl.style.color = zone.color;

  hrHistory.push(hr);
  if (hrHistory.length > MAX_POINTS) hrHistory.shift();
  drawGraph();
});

listen("connection-changed", (event) => {
  isConnected = event.payload;
  updateConnectionUI();
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
    document.querySelectorAll(".sidebar-icon").forEach((i) => i.classList.remove("active"));
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

// ── Always on top ──
document.getElementById("alwaysOnTopToggle").addEventListener("click", () => {
  const toggle = document.getElementById("alwaysOnTopToggle");
  const enabled = toggle.dataset.checked === "true";
  invoke("plugin:window|set_always_on_top", { label: "main", value: enabled });
  addLog(`Always on top: ${enabled ? "on" : "off"}`);
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
  { name: "Pill Badge", file: "pill", desc: "Minimal · Corner placement", size: "200×56" },
  { name: "Glass Card", file: "glass", desc: "Liquid Glass · Mini graph", size: "300×130" },
  { name: "Neon Ring", file: "neon", desc: "Cyberpunk · Circular progress", size: "192×192" },
  { name: "Full Widget", file: "widget", desc: "Full info · Graph + Stats", size: "340×160" },
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
        <iframe src="http://localhost:${port}/overlay/${o.file}" style="border:none;width:100%;height:100%;pointer-events:none;background:transparent" loading="lazy"></iframe>
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

// ── Init ──
loadOscParams();
updateConnectionUI();
addLog("SpoitableHRS initialized");
