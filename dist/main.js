const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);
const sysSel = $("sysSource"), micSel = $("micSource"), modelSel = $("model");
const startBtn = $("start"), stopBtn = $("stop"), clearBtn = $("clear");
const statusEl = $("status"), folderEl = $("folder"), transcriptEl = $("transcript");
const sendNowBtn = $("sendNow"), emailStatusEl = $("emailStatus"), emailEnabledEl = $("emailEnabled");
const sysOn = $("sysOn"), micOn = $("micOn");

let lines = []; // {time, who, text, channel}

// input-level VU meters (updated from backend "level" events)
const meters = { sys: { box: $("meterSys") }, mic: { box: $("meterMic") } };
for (const k of Object.keys(meters)) {
  meters[k].cover = meters[k].box.querySelector(".cover");
  meters[k].peak = meters[k].box.querySelector(".peak");
  meters[k].lbl = meters[k].box.querySelector(".lbl");
}
function setMeter(ch, level, peak) {
  const m = meters[ch]; if (!m) return;
  const lv = Math.max(0, Math.min(1, level));
  const pk = Math.max(0, Math.min(1, peak));
  m.cover.style.width = ((1 - lv) * 100).toFixed(1) + "%";
  m.peak.style.left = (pk * 100).toFixed(1) + "%";
  m.peak.style.opacity = pk > 0.02 ? "1" : "0";
}
function resetMeter(ch) {
  const m = meters[ch]; if (!m) return;
  m.cover.style.width = "100%"; m.peak.style.opacity = "0";
}

function opt(value, label) {
  const o = document.createElement("option");
  o.value = value; o.textContent = label;
  return o;
}

function setStatus(text, cls) {
  statusEl.textContent = text;
  statusEl.className = "status " + cls;
}

function showPlaceholder() {
  transcriptEl.innerHTML =
    '<div class="placeholder">A transcrição aparecerá aqui ao iniciar.</div>';
}

async function loadSources() {
  const [sources, def] = await Promise.all([
    invoke("list_sources"),
    invoke("defaults"),
  ]);

  sysSel.innerHTML = ""; micSel.innerHTML = "";

  for (const s of sources) {
    const tag = s.is_monitor ? " [monitor]" : "";
    sysSel.appendChild(opt(s.name, s.name + tag));
    micSel.appendChild(opt(s.name, s.name + tag));
  }
  // Preselect: system audio = default sink's monitor; mic = default source.
  if ([...sysSel.options].some((o) => o.value === def.sink_monitor))
    sysSel.value = def.sink_monitor;
  if ([...micSel.options].some((o) => o.value === def.default_source))
    micSel.value = def.default_source;
}

async function loadModels() {
  const models = await invoke("list_models");
  modelSel.innerHTML = "";
  if (!models.length) {
    modelSel.appendChild(opt("", "nenhum modelo — rode scripts/get-model.sh"));
    modelSel.disabled = true;
    startBtn.disabled = true;
    return;
  }
  modelSel.disabled = false;
  for (const m of models) modelSel.appendChild(opt(m, m));
  // Prefer large-v3 (best accuracy; the GPU handles it in real time), else medium.
  const pref =
    models.find((m) => m.includes("large-v3")) ||
    models.find((m) => m.includes("medium"));
  if (pref) modelSel.value = pref;
}

function render() {
  if (!lines.length) { showPlaceholder(); return; }
  const atBottom =
    transcriptEl.scrollHeight - transcriptEl.scrollTop - transcriptEl.clientHeight < 60;
  transcriptEl.innerHTML = "";
  for (const l of lines) {
    const div = document.createElement("div");
    div.className = "line " + l.channel;
    div.innerHTML =
      `<span class="t">${l.time}</span>` +
      `<span class="who">${l.who}</span>` +
      `<span class="txt"></span>`;
    div.querySelector(".txt").textContent = l.text;
    transcriptEl.appendChild(div);
  }
  if (atBottom) transcriptEl.scrollTop = transcriptEl.scrollHeight;
}

function addLine(line) {
  // Insert keeping chronological order (two channels can arrive interleaved).
  let i = lines.length;
  while (i > 0 && lines[i - 1].time > line.time) i--;
  lines.splice(i, 0, line);
  render();
}

startBtn.addEventListener("click", async () => {
  const opts = {
    sys_source: sysOn.checked && sysSel.value ? sysSel.value : null,
    mic_source: micOn.checked && micSel.value ? micSel.value : null,
    model: modelSel.value,
    language: $("language").value,
    segment_secs: parseInt($("segment").value, 10) || 10,
    sys_label: $("sysLabel").value || "OUTROS",
    mic_label: $("micLabel").value || "EU",
    email_enabled: emailEnabledEl.checked,
    email_to: $("emailTo").value.trim(),
    email_every_minutes: parseInt($("emailMinutes").value, 10) || 0,
    email_every_lines: parseInt($("emailLines").value, 10) || 0,
  };
  if (!opts.sys_source && !opts.mic_source) {
    setStatus("selecione uma fonte", "error");
    return;
  }
  if (opts.email_enabled) {
    if (!opts.email_to) {
      setStatus("informe o e-mail de destino", "error");
      return;
    }
    if (opts.email_every_minutes === 0 && opts.email_every_lines === 0) {
      setStatus("defina minutos ou linhas para o e-mail", "error");
      return;
    }
  }
  try {
    setStatus("carregando modelo…", "running");
    startBtn.disabled = true;
    const folder = await invoke("start_session", { opts });
    setStatus("● gravando + transcrevendo", "running");
    folderEl.textContent = "Sessão: " + folder;
    stopBtn.disabled = false;
    sendNowBtn.disabled = !opts.email_enabled;
    emailStatusEl.textContent = opts.email_enabled
      ? `e-mail automático → ${opts.email_to}`
      : "";
    emailStatusEl.className = "email-status";
    meters.sys.box.classList.toggle("off", !opts.sys_source);
    meters.mic.box.classList.toggle("off", !opts.mic_source);
    meters.sys.lbl.textContent = opts.sys_label;
    meters.mic.lbl.textContent = opts.mic_label;
    resetMeter("sys"); resetMeter("mic");
  } catch (e) {
    setStatus("erro", "error");
    folderEl.textContent = String(e);
    startBtn.disabled = false;
  }
});

stopBtn.addEventListener("click", async () => {
  stopBtn.disabled = true;
  sendNowBtn.disabled = true;
  try {
    const folder = await invoke("stop_session");
    folderEl.textContent = "Salvo em: " + folder + "/transcript.txt";
  } catch (e) {
    folderEl.textContent = String(e);
  }
  setStatus("parado", "idle");
  startBtn.disabled = false;
  meters.sys.box.classList.add("off");
  meters.mic.box.classList.add("off");
  resetMeter("sys"); resetMeter("mic");
});

sysOn.addEventListener("change", () => { sysSel.disabled = !sysOn.checked; });
micOn.addEventListener("change", () => { micSel.disabled = !micOn.checked; });

sendNowBtn.addEventListener("click", async () => {
  try {
    const msg = await invoke("send_email_now");
    emailStatusEl.textContent = msg;
    emailStatusEl.className = "email-status";
  } catch (e) {
    emailStatusEl.textContent = String(e);
    emailStatusEl.className = "email-status err";
  }
});

clearBtn.addEventListener("click", () => { lines = []; render(); });

listen("transcript", (ev) => addLine(ev.payload));
listen("error", (ev) => { setStatus("erro: " + ev.payload, "error"); });
listen("level", (ev) => setMeter(ev.payload.channel, ev.payload.level, ev.payload.peak));
listen("email_ok", (ev) => {
  emailStatusEl.textContent = "✓ " + ev.payload;
  emailStatusEl.className = "email-status ok";
});
listen("email_err", (ev) => {
  emailStatusEl.textContent = "⚠ " + ev.payload;
  emailStatusEl.className = "email-status err";
});

(async function init() {
  showPlaceholder();
  try {
    await loadSources();
    await loadModels();
  } catch (e) {
    folderEl.textContent = "Erro ao inicializar: " + e;
  }
})();
