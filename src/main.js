const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const bpmEl = document.getElementById("bpm");
const heartEl = document.getElementById("heart");
const scanBtn = document.getElementById("scanBtn");
const deviceList = document.getElementById("deviceList");
const connectBtn = document.getElementById("connectBtn");
const oscToggle = document.getElementById("oscToggle");
const oscPort = document.getElementById("oscPort");
const statusEl = document.getElementById("status");
const logContainer = document.getElementById("logContainer");
const clearLogBtn = document.getElementById("clearLogBtn");
const hrCanvas = document.getElementById("hrCanvas");

let isConnected = false;
let selectedDeviceId = null;
let beatTimeout = null;

// HR Graph
const MAX_POINTS = 120;
const hrHistory = [];
const HR_MIN = 40;
const HR_MAX = 200;

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

  // Grid lines
  ctx.strokeStyle = "#ffffff10";
  ctx.lineWidth = 1;
  for (const val of [60, 80, 100, 120, 140, 160, 180]) {
    const y = h - ((val - HR_MIN) / (HR_MAX - HR_MIN)) * h;
    ctx.beginPath();
    ctx.moveTo(0, y);
    ctx.lineTo(w, y);
    ctx.stroke();

    ctx.fillStyle = "#ffffff30";
    ctx.font = `${10 * devicePixelRatio}px sans-serif`;
    ctx.fillText(val, 4, y - 2);
  }

  // HR line
  const gradient = ctx.createLinearGradient(0, 0, 0, h);
  gradient.addColorStop(0, "#ff5555");
  gradient.addColorStop(0.5, "#e74c6f");
  gradient.addColorStop(1, "#3a86ff");

  ctx.strokeStyle = gradient;
  ctx.lineWidth = 2 * devicePixelRatio;
  ctx.lineJoin = "round";
  ctx.beginPath();

  const step = w / (MAX_POINTS - 1);
  const offset = MAX_POINTS - hrHistory.length;

  for (let i = 0; i < hrHistory.length; i++) {
    const x = (offset + i) * step;
    const y = h - ((hrHistory[i] - HR_MIN) / (HR_MAX - HR_MIN)) * h;
    if (i === 0) ctx.moveTo(x, y);
    else ctx.lineTo(x, y);
  }
  ctx.stroke();

  // Fill under curve
  const lastX = (offset + hrHistory.length - 1) * step;
  ctx.lineTo(lastX, h);
  ctx.lineTo(offset * step, h);
  ctx.closePath();

  const fillGrad = ctx.createLinearGradient(0, 0, 0, h);
  fillGrad.addColorStop(0, "#e74c6f18");
  fillGrad.addColorStop(1, "#e74c6f02");
  ctx.fillStyle = fillGrad;
  ctx.fill();
}

initCanvas();
window.addEventListener("resize", () => {
  initCanvas();
  drawGraph();
});

function addLog(message, level = "info") {
  const entry = document.createElement("div");
  entry.className = `log-entry ${level}`;
  const now = new Date();
  const time = now.toLocaleTimeString("ja-JP", { hour12: false });
  entry.innerHTML = `<span class="time">[${time}]</span> ${message}`;
  logContainer.appendChild(entry);
  logContainer.scrollTop = logContainer.scrollHeight;
}

// Tabs
document.querySelectorAll(".tab").forEach((tab) => {
  tab.addEventListener("click", () => {
    document.querySelectorAll(".tab").forEach((t) => t.classList.remove("active"));
    document.querySelectorAll(".tab-content").forEach((c) => c.classList.remove("active"));
    tab.classList.add("active");
    document.getElementById(`tab-${tab.dataset.tab}`).classList.add("active");
  });
});

clearLogBtn.addEventListener("click", () => {
  logContainer.innerHTML = "";
});

function animateBeat() {
  heartEl.classList.add("beat");
  clearTimeout(beatTimeout);
  beatTimeout = setTimeout(() => heartEl.classList.remove("beat"), 150);
}

listen("heart-rate-update", (event) => {
  const hr = event.payload;
  bpmEl.textContent = hr;
  animateBeat();

  hrHistory.push(hr);
  if (hrHistory.length > MAX_POINTS) hrHistory.shift();
  drawGraph();
});

listen("connection-changed", (event) => {
  isConnected = event.payload;
  updateUI();
  statusEl.textContent = isConnected ? "Connected" : "Disconnected";
  if (!isConnected) {
    hrHistory.length = 0;
    drawGraph();
  }
});

listen("ble-log", (event) => {
  const { message, level } = event.payload;
  addLog(message, level);
});

scanBtn.addEventListener("click", async () => {
  statusEl.textContent = "Scanning...";
  addLog("Starting BLE scan...");
  scanBtn.disabled = true;
  try {
    const devices = await invoke("scan_devices");
    deviceList.innerHTML = '<option value="">-- Select Device --</option>';
    devices.forEach((d) => {
      const opt = document.createElement("option");
      opt.value = d.id;
      opt.textContent = d.name;
      deviceList.appendChild(opt);
    });
    deviceList.disabled = false;
    statusEl.textContent = `Found ${devices.length} device(s)`;
    addLog(`Scan complete: ${devices.length} device(s) found`);
    devices.forEach((d) => addLog(`  ${d.name} (${d.id})`));
  } catch (e) {
    statusEl.textContent = `Scan failed: ${e}`;
    addLog(`Scan failed: ${e}`, "error");
  }
  scanBtn.disabled = false;
});

deviceList.addEventListener("change", () => {
  selectedDeviceId = deviceList.value || null;
  connectBtn.disabled = !selectedDeviceId;
});

connectBtn.addEventListener("click", async () => {
  if (isConnected) {
    addLog("Disconnecting...");
    await invoke("disconnect_device");
    bpmEl.textContent = "--";
    isConnected = false;
    updateUI();
    statusEl.textContent = "Disconnected";
    addLog("Disconnected");
  } else if (selectedDeviceId) {
    statusEl.textContent = "Connecting...";
    addLog(`Connecting to ${selectedDeviceId}...`);
    try {
      await invoke("connect_device", { deviceId: selectedDeviceId });
    } catch (e) {
      statusEl.textContent = `Connection failed: ${e}`;
      addLog(`Connection failed: ${e}`, "error");
    }
  }
});

oscToggle.addEventListener("change", () => {
  invoke("set_osc_enabled", { enabled: oscToggle.checked });
  addLog(`OSC output ${oscToggle.checked ? "enabled" : "disabled"}`);
});

oscPort.addEventListener("change", () => {
  const port = parseInt(oscPort.value, 10);
  if (port > 0 && port <= 65535) {
    invoke("set_osc_port", { port });
    addLog(`OSC port set to ${port}`);
  }
});

function updateUI() {
  connectBtn.textContent = isConnected ? "Disconnect" : "Connect";
  connectBtn.classList.toggle("connected", isConnected);
  scanBtn.disabled = isConnected;
  deviceList.disabled = isConnected;
}

addLog("SpoitableHRS initialized");
