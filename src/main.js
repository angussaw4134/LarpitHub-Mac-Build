import "./style.css";
import { invoke } from "@tauri-apps/api/core";
import { listen, emit } from "@tauri-apps/api/event";

const $ = id => document.getElementById(id);
const screens = ["connect", "account", "package"];
const libraryStep = -1;
let step = 0, busy = false, phones = [], selected = null, packageInfo = null;
let factorId = null, certificateId = null;
let usesFolderSlots = false;
let launchSlot = null;
const cards = [];

function status(text = "", type = "") {
  $("status").textContent = text;
  $("status").className = type;
}
function update() {
  const inLibrary = step === libraryStep;
  $("library").hidden = !inLibrary;
  $("loader-layout").classList.toggle("library-view", inLibrary);
  $("install-topline").hidden = inLibrary;
  $("back-to-apps").hidden = !usesFolderSlots || inLibrary;
  screens.forEach((name, i) => { $(name).hidden = i !== step; });
  [...$("steps").children].forEach((node, i) => {
    node.className = i < step ? "done" : i === step ? "active" : "";
    if (i === step) node.setAttribute("aria-current", "step"); else node.removeAttribute("aria-current");
  });
  $("step-count").textContent = "Step " + (step + 1) + " of 3";
  $("lock-state").textContent = step >= 2 ? "Signed in" : "Locked";
  $("lock-state").classList.toggle("open", step >= 2);
  $("phone-bar").hidden = !selected || step <= 0;
  $("phone-name").textContent = selected?.name || "";
  for (const el of document.querySelectorAll("main button, main select, main input")) el.disabled = busy;
  $("connect-phone").disabled = busy || !$("phones").value;
  $("install").disabled = busy || !packageInfo;
  for (const card of cards) card.button.disabled = busy || !card.title;
}
function go(next) { step = next; status(); update(); }
function explain(err) {
  const message = typeof err === "string" ? err : err?.message;
  if (!message) return "The operation failed. Try again.";
  if (/503|502|504/.test(message)) return "Apple sign-in is temporarily unavailable. Try again in a few minutes.";
  if (/429/.test(message)) return "Apple is limiting sign-in attempts. Wait before trying again.";
  return message.length < 600 ? message : "The operation failed. Check your connection and iPhone, then retry.";
}
async function run(action) {
  if (busy) return;
  busy = true; update(); status();
  try { await action(); } catch (err) { if (err?.type === "not_logged_in") { resetFile(); go(1); } status(explain(err), "error"); }
  finally { busy = false; update(); }
}
async function refresh() {
  phones = []; $("phones").replaceChildren(new Option("Choose an iPhone", ""));
  phones = await invoke("list_devices");
  for (const d of phones) $("phones").add(new Option(d.name + " · " + d.udid.slice(-4), d.udid));
  if (phones.length === 1) {
    const phone = phones[0];
    $("phones").value = phone.udid;
    await connectPhone(phone);
  }
  if (!phones.length) status("No iPhone detected. Connect it by USB, then click Refresh.");
}
async function connectPhone(phone) {
  await invoke("select_phone", {udid: phone.udid});
  await invoke("check_phone");
  selected = phone;
  go(1);
  if (await invoke("restore_apple")) await showInstall();
}
async function showInstall() {
  go(2);
  if (launchSlot !== null) await loadPackage(launchSlot);
}
$("refresh").onclick = () => run(refresh);
$("phones").onchange = update;
$("connect-phone").onclick = () => run(async () => {
  const phone = phones.find(d => d.udid === $("phones").value);
  if (!phone) return;
  await connectPhone(phone);
});
$("change").onclick = () => run(async () => {
  await invoke("logout_apple"); await invoke("select_phone", {udid:null});
  selected = null; resetFile(); go(0); await refresh();
});
function resetFile() {
  packageInfo = null;
  $("file-name").textContent = "No IPA selected";
  $("file-info").textContent = usesFolderSlots ? "Choose an app from your library" : "Select a local .ipa file";
}
$("login-form").onsubmit = e => {
  e.preventDefault();
  run(async () => {
    status("Contacting Apple…");
    const password = $("password").value; $("password").value = "";
    try {
      await invoke("login_apple", {email: $("email").value.trim(), password, server: $("server").value});
      await showInstall();
    }
    finally { factorId = null; $("two-factor").close(); }
  });
};
$("signout").onclick = () => run(async () => { await invoke("logout_apple"); resetFile(); go(1); });
$("choose").onclick = () => run(() => loadPackage(1));
$("choose-second").onclick = () => run(() => loadPackage(2));
$("back-to-apps").onclick = () => run(async () => {
  launchSlot = null;
  resetFile();
  go(libraryStep);
  await readLibrary();
});
$("reload-slots").onclick = () => run(readLibrary);
$("choose-folder").onclick = () => run(async () => {
  if (await invoke("choose_ipa_folder")) {
    resetFile();
    await readLibrary();
    status("IPA folder saved.");
  }
});

function createLibrary() {
  for (const slot of [1, 2, 3]) {
    const element = $("slot-template").content.firstElementChild.cloneNode(true);
    const button = element.querySelector(".launch-app");
    const cover = element.querySelector("img");
    cover.onerror = () => { cover.hidden = true; };
    button.onclick = () => run(async () => {
      launchSlot = slot;
      resetFile();
      go(0);
      await refresh();
    });
    cards.push({
      element, cover, button, title: null,
      heading: element.querySelector(".app-title"),
      location: element.querySelector(".app-location"),
      issue: element.querySelector(".app-issue")
    });
    $("app-slots").append(element);
  }
}

async function readLibrary() {
  for (const card of cards) card.title = null;
  const slots = await invoke("read_ipa_slots");
  cards.forEach((card, index) => {
    const slot = slots[index] || {};
    const folder = "ipa" + (index + 1);
    card.title = slot.title || null;
    card.heading.textContent = card.title || "Empty slot";
    card.location.textContent = card.title ? folder : "Add an IPA to " + folder;
    card.issue.textContent = slot.issue || "";
    card.issue.hidden = !slot.issue;
    card.element.classList.toggle("has-app", Boolean(card.title));
    card.button.setAttribute("aria-label", "Launch " + (card.title || folder));
    card.cover.hidden = !slot.artwork;
    if (slot.artwork) card.cover.src = slot.artwork;
    else card.cover.removeAttribute("src");
  });
}

async function loadPackage(slot) {
  if (usesFolderSlots) status("Checking the IPA…");
  const result = await invoke("choose_package", {slot});
  if (result) { packageInfo = result; $("file-name").textContent = result.name; $("file-info").textContent = [result.version, result.bundle].filter(Boolean).join(" · "); }
  status();
}
$("install").onclick = () => run(async () => {
  try { await invoke("install_package"); rememberInstall(packageInfo); status(packageInfo.name + " was installed on " + selected.name + ".", "success"); }
  finally { resetFile(); certificateId = null; $("certificates").close(); }
});

async function answerFactor(value) {
  if (!factorId) return;
  const id = factorId; factorId = null; $("two-factor").close(); $("code").value = "";
  await emit("accesshub-2fa-" + id, value);
}
async function answerCertificate(value) {
  if (!certificateId) return;
  const id = certificateId; certificateId = null; $("certificates").close();
  await emit("accesshub-certificates-" + id, value);
}
$("factor-form").onsubmit = e => {e.preventDefault(); answerFactor({SubmitCode: $("code").value.trim()});};
$("factor-cancel").onclick = () => answerFactor("Abort");
$("two-factor").oncancel = e => {e.preventDefault(); answerFactor("Abort");};
$("cert-cancel").onclick = () => answerCertificate(null);
$("certificates").oncancel = e => {e.preventDefault(); answerCertificate(null);};
$("cert-confirm").onclick = () => {
  const selected = [...$("cert-options").querySelectorAll("input:checked")].map(el => el.value);
  if (selected.length) answerCertificate(selected);
};
const reminderKey = "highzone.renewals.v1";
function renewals() {
  try { return JSON.parse(localStorage.getItem(reminderKey)) || {}; } catch { return {}; }
}
function rememberInstall(info) {
  const records = renewals();
  records[info.bundle] = {name:info.name, due:Date.now() + 7 * 86400000};
  localStorage.setItem(reminderKey, JSON.stringify(records));
  showRenewals();
}
function showRenewals() {
  const entries = Object.values(renewals()).sort((a,b) => a.due-b.due);
  const schedule = entries.map(({name,due}) => {
    const minutes = Math.ceil((due-Date.now())/60000);
    if (minutes <= 0) return name + " · Reload due";
    return name + " · Reload in " + Math.floor(minutes/1440) + "d " + Math.floor(minutes%1440/60) + "h " + minutes%60 + "m";
  }).join("\n");
  for (const id of ["renewal", "library-renewal"]) {
    $(id).hidden = !entries.length;
    $(id).textContent = schedule;
  }
}
async function start() {
  showRenewals(); setInterval(showRenewals, 30000);
  update();
  await listen("accesshub-2fa", ({payload:{requestId, params}}) => {
    factorId = requestId; $("code").value = "";
    $("factor-note").textContent = params.lastError || "Enter the six-digit code sent by Apple.";
    const container = $("factor-methods"); container.replaceChildren();
    function method(label, value) {
      const b = document.createElement("button"); b.className = "quiet"; b.textContent = label;
      b.onclick = () => answerFactor(value); container.append(b);
    }
    method("Resend code", "ResendCode"); method("Send to devices", "SendToDevices");
    for (const number of params.numbers || []) method("Text " + (number.numberWithDialCode || number.number || "trusted phone"), {SendSms: number.id});
    $("two-factor").showModal(); $("code").focus();
  });
  await listen("accesshub-certificates", ({payload:{requestId, params}}) => {
    certificateId = requestId; $("cert-options").replaceChildren(); $("cert-confirm").disabled = true;
    for (const cert of params) {
      const label = document.createElement("label"), input = document.createElement("input");
      input.type = "checkbox"; input.value = cert.serial;
      input.onchange = () => { $("cert-confirm").disabled = !$("cert-options").querySelector("input:checked"); };
      label.append(input, document.createTextNode(cert.name)); $("cert-options").append(label);
    }
    $("certificates").showModal();
  });
  await listen("install-stage", ({payload}) => status(payload));
  const [hasSecond, hasFolders] = await Promise.all([invoke("has_second_package"), invoke("uses_ipa_folders")]);
  usesFolderSlots = hasFolders;
  $("choose").hidden = hasFolders;
  $("choose-second").hidden = !hasSecond;
  if (hasSecond) $("choose").textContent = "Select app 1";
  resetFile();
  if (hasFolders) {
    $("package-description").textContent = "Check the app details before installing it on your iPhone.";
    createLibrary();
    go(libraryStep);
    await run(readLibrary);
    return;
  }
  await run(refresh);
}
start().catch(e => status(explain(e), "error"));
