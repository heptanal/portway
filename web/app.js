"use strict";

const PROTOCOL_VERSION = 1;
const HEARTBEAT_MS = 5000;
const INACTIVE_RELEASE_MS = 30000;
const MOVE_THRESHOLD = 8;
const POINTER_FRAME_INTERVAL_MS = 1000 / 60;
const POINTER_FRAME_TOLERANCE_MS = 1;
const TRANSIENT_BUFFER_LIMIT_BYTES = 1024;

const ui = {
  authDialog: document.querySelector("#auth-dialog"),
  authForm: document.querySelector("#auth-form"),
  pairInput: document.querySelector("#pair-input"),
  pairSubmit: document.querySelector("#pair-submit"),
  connection: document.querySelector("#connection"),
  statusText: document.querySelector("#status-text"),
  releaseAll: document.querySelector("#release-all"),
  touchpad: document.querySelector("#touchpad"),
  backendWarning: document.querySelector("#backend-warning"),
  dragLock: document.querySelector("#drag-lock"),
  textInput: document.querySelector("#text-input"),
  sensitivity: document.querySelector("#sensitivity"),
  sensitivityValue: document.querySelector("#sensitivity-value"),
  toast: document.querySelector("#toast"),
};

let socket = null;
let sequence = 0;
let reconnectTimer = null;
let reconnectAttempt = 0;
let connected = false;
let hasSession = false;
let lastActivity = Date.now();
let sensitivity = Number(localStorage.getItem("portway-sensitivity")) || 1;
let dragLocked = false;
const activeModifiers = new Set();
const modifierPresses = new Map();

ui.sensitivity.value = String(sensitivity);
ui.sensitivityValue.value = `${sensitivity.toFixed(1)}×`;

function setStatus(label, state = "") {
  ui.statusText.textContent = label;
  ui.connection.className = `connection ${state}`.trim();
}

function showToast(message) {
  ui.toast.textContent = message;
  ui.toast.hidden = false;
  clearTimeout(showToast.timer);
  showToast.timer = setTimeout(() => {
    ui.toast.hidden = true;
  }, 2600);
}

function showAuth() {
  if (!ui.authDialog.open) ui.authDialog.showModal();
  setTimeout(() => ui.pairInput.focus(), 50);
}

function connect() {
  clearTimeout(reconnectTimer);
  if (!hasSession) {
    showAuth();
    return;
  }
  if (socket && (socket.readyState === WebSocket.OPEN || socket.readyState === WebSocket.CONNECTING)) return;

  setStatus(reconnectAttempt ? "Reconnecting" : "Connecting");
  const scheme = location.protocol === "https:" ? "wss" : "ws";
  socket = new WebSocket(`${scheme}://${location.host}/ws`);

  socket.addEventListener("open", () => {
    sequence = 0;
    reconnectAttempt = 0;
    lastActivity = Date.now();
  });

  socket.addEventListener("message", (message) => {
    let data;
    try {
      data = JSON.parse(message.data);
    } catch {
      setStatus("Protocol error", "error");
      return;
    }
    if (data.type === "ready") {
      connected = true;
      setStatus(data.input_available ? "Connected" : "No input", data.input_available ? "connected" : "error");
      ui.backendWarning.hidden = data.input_available;
      if (ui.authDialog.open) ui.authDialog.close();
    } else if (data.type === "error") {
      showToast(data.message || "Server rejected the command");
      if (data.code === "input_unavailable") ui.backendWarning.hidden = false;
    }
  });

  socket.addEventListener("close", () => {
    connected = false;
    clearLocalHeldState();
    if (!hasSession) return;
    reconnectAttempt += 1;
    if (reconnectAttempt >= 4) {
      hasSession = false;
      setStatus("Pairing required", "error");
      showAuth();
      return;
    }
    setStatus("Reconnecting");
    reconnectTimer = setTimeout(connect, Math.min(8000, 500 * 2 ** reconnectAttempt));
  });

  socket.addEventListener("error", () => setStatus("Unavailable", "error"));
}

async function exchangeCredential(code) {
  const response = await fetch("/api/pair", {
    method: "POST",
    credentials: "same-origin",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ code }),
  });
  if (!response.ok) {
    if (response.status === 429) throw new Error("Too many attempts. Wait one minute.");
    if (response.status === 401) throw new Error("Pairing code or setup token rejected.");
    throw new Error("Pairing request failed.");
  }
  const result = await response.json();
  if (!result.authenticated) throw new Error("Pairing request failed.");
  hasSession = true;
}

async function initialize() {
  try {
    const response = await fetch("/api/session", {
      credentials: "same-origin",
      cache: "no-store",
    });
    if (!response.ok) throw new Error("session status unavailable");
    const session = await response.json();
    hasSession = session.authenticated;
    if (hasSession) {
      connect();
    } else {
      setStatus("Pairing required", "error");
      showAuth();
    }
  } catch {
    setStatus("Server unavailable", "error");
    reconnectTimer = setTimeout(initialize, 2000);
  }
}

function send(event, { transient = false } = {}) {
  if (event.type !== "heartbeat") lastActivity = Date.now();
  if (!socket || socket.readyState !== WebSocket.OPEN || !connected) return false;
  if (transient && socket.bufferedAmount > TRANSIENT_BUFFER_LIMIT_BYTES) return false;
  sequence += 1;
  socket.send(JSON.stringify({ v: PROTOCOL_VERSION, seq: sequence, event }));
  return true;
}

function clickButton(button) {
  send({ type: "pointer_button", button, state: "down" });
  setTimeout(() => send({ type: "pointer_button", button, state: "up" }), 35);
}

function tapKey(code) {
  send({ type: "key", code, state: "down" });
  setTimeout(() => send({ type: "key", code, state: "up" }), 32);
}

function clearLocalHeldState() {
  activeModifiers.clear();
  modifierPresses.clear();
  document.querySelectorAll(".modifier.active").forEach((button) => {
    button.classList.remove("active");
    button.setAttribute("aria-pressed", "false");
  });
  dragLocked = false;
  ui.dragLock.classList.remove("active");
  ui.dragLock.setAttribute("aria-pressed", "false");
}

function releaseEverything(notify = true) {
  send({ type: "release_all" });
  clearLocalHeldState();
  if (notify) showToast("All held input released");
}

ui.authForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  const code = ui.pairInput.value.trim();
  if (!code) return;
  ui.pairSubmit.disabled = true;
  setStatus("Pairing");
  try {
    await exchangeCredential(code);
    ui.pairInput.value = "";
    reconnectAttempt = 0;
    if (ui.authDialog.open) ui.authDialog.close();
    if (socket) socket.close();
    connect();
  } catch (error) {
    setStatus("Pairing required", "error");
    showToast(error.message);
  } finally {
    ui.pairSubmit.disabled = false;
  }
});

ui.connection.addEventListener("click", async () => {
  hasSession = false;
  if (socket) socket.close();
  try {
    await fetch("/api/session/logout", {
      method: "POST",
      credentials: "same-origin",
    });
  } catch {
    showToast("Server logout failed; the session may remain active.");
  }
  ui.pairInput.value = "";
  setStatus("Pairing required", "error");
  showAuth();
});

ui.releaseAll.addEventListener("click", () => releaseEverything());

document.querySelectorAll("[data-mode]").forEach((button) => {
  button.addEventListener("click", () => {
    const keyboard = button.dataset.mode === "keyboard";
    document.querySelector("#touch-mode").hidden = keyboard;
    document.querySelector("#keyboard-mode").hidden = !keyboard;
    document.querySelectorAll("[data-mode]").forEach((candidate) => {
      const active = candidate === button;
      candidate.classList.toggle("active", active);
      candidate.setAttribute("aria-pressed", String(active));
    });
    if (keyboard) setTimeout(() => ui.textInput.focus(), 80);
  });
});

ui.sensitivity.addEventListener("input", () => {
  sensitivity = Number(ui.sensitivity.value);
  ui.sensitivityValue.value = `${sensitivity.toFixed(1)}×`;
  localStorage.setItem("portway-sensitivity", String(sensitivity));
});

document.querySelectorAll(".mouse-button[data-button]").forEach((button) => {
  const mouseButton = button.dataset.button;
  const release = () => {
    button.classList.remove("active");
    send({ type: "pointer_button", button: mouseButton, state: "up" });
  };
  button.addEventListener("pointerdown", (event) => {
    event.preventDefault();
    button.setPointerCapture(event.pointerId);
    button.classList.add("active");
    send({ type: "pointer_button", button: mouseButton, state: "down" });
  });
  button.addEventListener("pointerup", release);
  button.addEventListener("pointercancel", release);
});

ui.dragLock.addEventListener("click", () => {
  dragLocked = !dragLocked;
  ui.dragLock.classList.toggle("active", dragLocked);
  ui.dragLock.setAttribute("aria-pressed", String(dragLocked));
  send({ type: "pointer_button", button: "left", state: dragLocked ? "down" : "up" });
});

const pointers = new Map();
let gestureMaxPointers = 0;
let gestureMovement = 0;
let longPressTimer = null;
let longPressDragging = false;
let pendingDx = 0;
let pendingDy = 0;
let pendingScrollX = 0;
let pendingScrollY = 0;
let frameRequested = false;
let lastPointerFrameAt = -Infinity;

function flushPointerFrame(timestamp) {
  if (timestamp - lastPointerFrameAt + POINTER_FRAME_TOLERANCE_MS < POINTER_FRAME_INTERVAL_MS) {
    requestAnimationFrame(flushPointerFrame);
    return;
  }

  frameRequested = false;
  lastPointerFrameAt = timestamp;
  const dx = Math.trunc(pendingDx * sensitivity);
  const dy = Math.trunc(pendingDy * sensitivity);
  pendingDx -= dx / sensitivity;
  pendingDy -= dy / sensitivity;
  if (dx || dy) {
    send(
      { type: "pointer_move", dx: Math.max(-2048, Math.min(2048, dx)), dy: Math.max(-2048, Math.min(2048, dy)) },
      { transient: true },
    );
  }

  const scrollX = Math.trunc(pendingScrollX / 12);
  const scrollY = Math.trunc(pendingScrollY / 12);
  pendingScrollX -= scrollX * 12;
  pendingScrollY -= scrollY * 12;
  if (scrollX || scrollY) {
    send(
      { type: "scroll", dx: Math.max(-120, Math.min(120, scrollX)), dy: Math.max(-120, Math.min(120, scrollY)) },
      { transient: true },
    );
  }
}

function requestPointerFrame() {
  if (!frameRequested) {
    frameRequested = true;
    requestAnimationFrame(flushPointerFrame);
  }
}

ui.touchpad.addEventListener("contextmenu", (event) => event.preventDefault());
ui.touchpad.addEventListener("pointerdown", (event) => {
  event.preventDefault();
  ui.touchpad.setPointerCapture(event.pointerId);
  pointers.set(event.pointerId, {
    x: event.clientX,
    y: event.clientY,
    startX: event.clientX,
    startY: event.clientY,
    started: performance.now(),
  });
  gestureMaxPointers = Math.max(gestureMaxPointers, pointers.size);
  if (pointers.size === 1) {
    gestureMovement = 0;
    longPressTimer = setTimeout(() => {
      if (pointers.size === 1 && gestureMovement < MOVE_THRESHOLD && !dragLocked) {
        longPressDragging = true;
        send({ type: "pointer_button", button: "left", state: "down" });
      }
    }, 430);
  } else {
    clearTimeout(longPressTimer);
  }
});

ui.touchpad.addEventListener("pointermove", (event) => {
  const point = pointers.get(event.pointerId);
  if (!point) return;
  event.preventDefault();
  const dx = event.clientX - point.x;
  const dy = event.clientY - point.y;
  gestureMovement += Math.abs(dx) + Math.abs(dy);
  point.x = event.clientX;
  point.y = event.clientY;
  if (gestureMovement >= MOVE_THRESHOLD) clearTimeout(longPressTimer);

  if (pointers.size === 1) {
    pendingDx += dx;
    pendingDy += dy;
  } else {
    pendingScrollX += dx / pointers.size;
    pendingScrollY += dy / pointers.size;
  }
  requestPointerFrame();
});

function finishPointer(event) {
  const point = pointers.get(event.pointerId);
  if (!point) return;
  event.preventDefault();
  const duration = performance.now() - point.started;
  pointers.delete(event.pointerId);
  clearTimeout(longPressTimer);

  if (longPressDragging) {
    if (pointers.size === 0) {
      send({ type: "pointer_button", button: "left", state: "up" });
      longPressDragging = false;
    }
  } else if (pointers.size === 0 && duration < 280 && gestureMovement < MOVE_THRESHOLD) {
    clickButton(gestureMaxPointers >= 2 ? "right" : "left");
  }
  if (pointers.size === 0) {
    gestureMaxPointers = 0;
    gestureMovement = 0;
  }
}

ui.touchpad.addEventListener("pointerup", finishPointer);
ui.touchpad.addEventListener("pointercancel", finishPointer);

document.querySelectorAll("[data-modifier]").forEach((button) => {
  const code = button.dataset.modifier;
  button.addEventListener("pointerdown", (event) => {
    event.preventDefault();
    button.setPointerCapture(event.pointerId);
    if (activeModifiers.has(code)) {
      send({ type: "key", code, state: "up" });
      activeModifiers.delete(code);
      button.classList.remove("active");
      button.setAttribute("aria-pressed", "false");
      modifierPresses.delete(event.pointerId);
      return;
    }
    send({ type: "key", code, state: "down" });
    activeModifiers.add(code);
    button.classList.add("active");
    button.setAttribute("aria-pressed", "true");
    modifierPresses.set(event.pointerId, { code, button, started: performance.now() });
  });

  const finish = (event) => {
    const press = modifierPresses.get(event.pointerId);
    if (!press) return;
    modifierPresses.delete(event.pointerId);
    if (performance.now() - press.started >= 350) {
      send({ type: "key", code: press.code, state: "up" });
      activeModifiers.delete(press.code);
      press.button.classList.remove("active");
      press.button.setAttribute("aria-pressed", "false");
    }
  };
  button.addEventListener("pointerup", finish);
  button.addEventListener("pointercancel", finish);
});

document.querySelectorAll("[data-key]").forEach((button) => {
  button.addEventListener("click", () => tapKey(button.dataset.key));
});

const functionKeys = Array.from({ length: 12 }, (_, index) => [`F${index + 1}`, `f${index + 1}`]);
const mediaKeys = [
  ["Mute", "volume_mute"],
  ["Vol −", "volume_down"],
  ["Vol +", "volume_up"],
  ["Previous", "media_previous"],
  ["Play / pause", "media_play_pause"],
  ["Next", "media_next"],
];

function addKeys(container, keys) {
  for (const [label, code] of keys) {
    const button = document.createElement("button");
    button.className = "key";
    button.type = "button";
    button.textContent = label;
    button.addEventListener("click", () => tapKey(code));
    container.append(button);
  }
}

addKeys(document.querySelector("#function-keys"), functionKeys);
addKeys(document.querySelector("#media-keys"), mediaKeys);

ui.textInput.addEventListener("beforeinput", (event) => {
  if (event.inputType === "insertText" || event.inputType === "insertFromPaste") {
    if (event.data) {
      event.preventDefault();
      send({ type: "text_input", text: event.data.slice(0, 128) });
    }
  } else if (event.inputType.startsWith("delete")) {
    event.preventDefault();
    tapKey(event.inputType.includes("Forward") ? "delete" : "backspace");
  } else if (event.inputType === "insertLineBreak" || event.inputType === "insertParagraph") {
    event.preventDefault();
    tapKey("enter");
  }
  ui.textInput.value = "";
});

ui.textInput.addEventListener("compositionend", (event) => {
  if (event.data) send({ type: "text_input", text: event.data.slice(0, 128) });
  ui.textInput.value = "";
});

setInterval(() => {
  if (connected) send({ type: "heartbeat" });
  if (Date.now() - lastActivity > INACTIVE_RELEASE_MS && (activeModifiers.size || dragLocked || longPressDragging)) {
    releaseEverything(false);
  }
}, HEARTBEAT_MS);

document.addEventListener("visibilitychange", () => {
  if (document.hidden) releaseEverything(false);
});
window.addEventListener("pagehide", () => releaseEverything(false));
window.addEventListener("pointerdown", () => { lastActivity = Date.now(); }, { passive: true });
window.addEventListener("keydown", () => { lastActivity = Date.now(); }, { passive: true });

initialize();
